//! From-scratch ODE integration for `tpt-sci-ode`.
//!
//! Four methods are provided, all implemented in-house on top of the in-crate
//! dense linear algebra (no `diffsol`/`nalgebra`/`faer` in the shipped graph):
//!
//! * [`Method::Tsit45`] — explicit Runge–Kutta 5(4) (Dormand–Prince),
//!   non-stiff.
//! * [`Method::TrBdf2`] — TR-BDF2 (3-stage ESDIRK form), A-/L-stable, stiff.
//! * [`Method::Esdirk34`] — 4-stage ESDIRK order 3(4), A-/L-stable, stiff.
//! * [`Method::Bdf`] — variable-order (1–5) backward differentiation, stiff.
//!
//! A shared adaptive-step driver (`integrate`) handles step-size control and
//! dense output via Hermite interpolation.

use crate::error::OdeError;
use crate::events::{Direction, Event, EventHit, EventSolution, bisect_root};
use crate::linalg::{DMat, NEWTON_TOL, RhsFn, Tol, eval, jacobian, sdirk_stage};
use crate::problem::OdeProblem;
use crate::sparse::JacobianPattern;

/// Integration method selection for [`OdeProblem::solve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Backward Differentiation Formulae — variable-order (1–5), stiff-capable,
    /// L-stable. The default for general problems.
    Bdf,
    /// Explicit Runge–Kutta 5(4) (Dormand–Prince), non-stiff.
    Tsit45,
    /// Trapezoidal-rule BDF2 (SDIRK/ESDIRK family) — stiff-capable, A-/L-stable.
    TrBdf2,
    /// Explicit-first-stage singly-diagonally-implicit RK of order 3(4) — stiff-
    /// capable, A-/L-stable.
    Esdirk34,
}

impl Method {
    /// Exponent `q` of the local error in the step-size controller: the local
    /// error of one step is `O(h^q)`, so the controller scales `h` by
    /// `(target / err)^(1/q)`.
    fn error_order(self, bdf_order: usize) -> f64 {
        match self {
            Method::Tsit45 => 5.0,                 // 5(4): local error ~ h^5
            Method::TrBdf2 => 3.0,                 // trapezoid vs TR-BDF2 difference ~ h^3
            Method::Esdirk34 => 4.0,               // 3(4): local error ~ h^4
            Method::Bdf => bdf_order as f64 + 1.0, // BDF-k: LTE ~ h^{k+1}
        }
    }
}

/// Result of a single successful step.
struct StepResult {
    /// New time.
    t: f64,
    /// New state.
    y: Vec<f64>,
    /// Per-component local error estimate (at the new state).
    err: Vec<f64>,
    /// Derivative at the end of the step.
    f_new: Vec<f64>,
}

/// Attempt one step of the given method; returns `Ok` on success with the new
/// state and a local error estimate, or `Err` if the nonlinear solve could not
/// converge (caller should shrink `h` and retry). `bdf_state`, if `Some`, is the
/// BDF order/history; it is updated in place on success.
#[allow(clippy::too_many_arguments)]
fn try_step(
    method: Method,
    f: &dyn crate::RhsCallable,
    t: f64,
    y: &[f64],
    h: f64,
    tol: Tol,
    jac: Option<&JacobianPattern>,
    bdf_state: Option<&mut NordsieckState>,
) -> Result<StepResult, OdeError> {
    match method {
        Method::Tsit45 => step_dp54(f, t, y, h),
        Method::TrBdf2 => step_sdirk2(f, t, y, h, tol, jac),
        Method::Esdirk34 => step_esdirk34(f, t, y, h, tol, jac),
        Method::Bdf => step_bdf(
            f,
            t,
            y,
            h,
            tol,
            jac,
            bdf_state.expect("BDF requires a Nordsieck state"),
        ),
    }
}

/// Adaptive-step driver shared by `solve` and `solve_dense`.
/// Result of one call to [`integrate`].
struct Integration {
    /// Recorded states at the `t_eval` points (empty when no `t_eval` was given).
    outputs: Vec<Vec<f64>>,
    /// Time where integration stopped.
    t_end: f64,
    /// State at `t_end`.
    y_end: Vec<f64>,
    /// Event crossings, in time order.
    hits: Vec<EventHit>,
}

fn integrate(
    prob: &OdeProblem,
    method: Method,
    t_final: f64,
    t_eval: Option<&[f64]>,
    events: &[Event],
) -> Result<Integration, OdeError> {
    let f = &*prob.rhs;
    let rtol = prob.rtol;
    let atol = prob.atol;
    let tol = Tol { rtol, atol };

    let mut t = prob.t0;
    let mut y = prob.y0.clone();
    let mut f_cur = eval(f, t, &y)?;
    let dir = if t_final >= t { 1.0 } else { -1.0 };
    let span = (t_final - t).abs();

    let h_min = 1e-14_f64.max(span * 1e-15);
    let h_max = span; // never step past the whole interval at once
    let max_steps = 100_000usize;
    let safety = 0.9_f64;

    // Initial step: the caller's `h0` if given, otherwise a heuristic fraction of
    // the interval. Either way it is clamped to the step limits.
    let h_init = prob.h0.unwrap_or(span * 1e-2).abs().max(1e-6).min(h_max);
    let mut h = if dir < 0.0 { -h_init } else { h_init };

    let mut bdf_state = if method == Method::Bdf {
        let mut ns = NordsieckState::new(prob.nstates());
        // Initialize at order 1 (backward Euler) using the first step h.
        ns.initialize(&prob.y0, &f_cur, h);
        Some(ns)
    } else {
        None
    };
    // Steps taken at the current order, used to gate BDF order changes.
    let mut bdf_steps_at_order = 0usize;

    // Output bookkeeping.
    let mut outputs: Vec<Vec<f64>> = Vec::new();
    let mut eval_idx = 0usize;
    if let Some(te) = t_eval {
        // Every point must lie beyond t0 and each must follow the previous one in
        // the direction of integration. `!(x > 0)` also rejects NaN.
        let mut prev = t;
        for &tev in te {
            let step = dir * (tev - prev);
            if step.is_nan() || step <= 0.0 {
                return Err(OdeError::invalid(
                    "t_eval entries must be finite, strictly increasing, and beyond t0",
                ));
            }
            prev = tev;
        }
    }

    let mut hits: Vec<EventHit> = Vec::new();
    let mut steps = 0usize;
    loop {
        let h_to_final = dir * (t_final - t);
        if h_to_final <= 0.0 {
            break;
        }
        // When a step is clamped onto a target time, remember the target so the
        // accepted step can be snapped onto it exactly (see below).
        let mut snap: Option<f64> = None;
        if h.abs() > h_to_final {
            h = dir * h_to_final;
            snap = Some(t_final);
        }
        if let Some(te) = t_eval {
            if eval_idx < te.len() {
                let h_to_eval = dir * (te[eval_idx] - t);
                if h.abs() > h_to_eval {
                    h = dir * h_to_eval;
                    snap = Some(te[eval_idx]);
                }
            }
        }
        if h.abs() < h_min {
            return Err(OdeError::StepTooSmall { t });
        }

        let q = method.error_order(bdf_state.as_ref().map(|s| s.order).unwrap_or(1));
        // Snapshot the BDF history so a *rejected* step (which still mutates the
        // Nordsieck vector inside `step_bdf`) cannot corrupt the integration.
        // On rejection we restore this snapshot before retrying.
        let bdf_snapshot = bdf_state.clone();
        match try_step(
            method,
            f,
            t,
            &y,
            h,
            tol,
            prob.jac_pattern.as_ref(),
            bdf_state.as_mut(),
        ) {
            Ok(mut res) => {
                // `t + h` can land an ulp short of the target, which would leave a
                // sliver of remaining interval and a step below `h_min`. The step
                // was sized to end exactly on the target, so take that time as-is.
                if let Some(target) = snap {
                    res.t = target;
                }
                let err_est = weighted_norm(&res.err, &res.y, rtol, atol);
                // A step is accepted only if it meets the tolerance. When the step
                // is already at its minimum, rejection leads to `StepTooSmall` below
                // rather than a silently inaccurate result.
                let accept = err_est <= 1.0;
                if !accept {
                    // Reject: restore the snapshot, shrink h, and retry without
                    // advancing. This keeps the BDF history consistent across
                    // retries (the previous behaviour let rejected steps scramble
                    // the Nordsieck columns, which eventually produced garbage).
                    if let (Some(ns), Some(saved)) = (bdf_state.as_mut(), &bdf_snapshot) {
                        *ns = saved.clone();
                    }
                    let mut next = h * safety * 0.2_f64.max(err_est.powf(-1.0 / q));
                    if dir < 0.0 {
                        next = -next.abs();
                    }
                    if next.abs() < h_min {
                        return Err(OdeError::StepTooSmall { t });
                    }
                    h = next;
                    continue;
                }

                // Accept: grow step for next time, record outputs, advance.
                // Limit step size growth to at most 1.5x per step to prevent instability.
                //
                // The step-size controller targets a weighted local error of
                // ~1.0 for the explicit/SDIRK methods. For BDF we deliberately
                // target a smaller value (~0.4) so there is headroom to raise
                // the method order when profitable.
                let target = if method == Method::Bdf { 0.4 } else { 1.0 };
                let growth_factor = if err_est < 1e-12 {
                    1.5_f64
                } else {
                    (target / err_est).powf(1.0 / q) * safety
                }
                .min(5.0)
                .min(1.5);
                let mut next = h * growth_factor;
                if dir < 0.0 {
                    next = -next.abs();
                }
                next = dir * next.abs().min(h_max);

                // Locate event crossings inside this step. A terminal event
                // ends the integration at its crossing time.
                if let Some((te, y_te, f_te)) =
                    detect_events(events, f, t, &y, &f_cur, &res, &mut hits)?
                {
                    let stop = StepResult {
                        t: te,
                        y: y_te.clone(),
                        err: Vec::new(),
                        f_new: f_te,
                    };
                    record_outputs(
                        t,
                        &y,
                        &f_cur,
                        &stop,
                        t_eval,
                        &mut eval_idx,
                        dir,
                        &mut outputs,
                    );
                    return Ok(Integration {
                        outputs,
                        t_end: te,
                        y_end: y_te,
                        hits,
                    });
                }

                record_outputs(
                    t,
                    &y,
                    &f_cur,
                    &res,
                    t_eval,
                    &mut eval_idx,
                    dir,
                    &mut outputs,
                );

                if let Some(ns) = bdf_state.as_mut() {
                    // Order control. Variable-order BDF: raise the order when
                    // the step is comfortably accurate for several steps; lower
                    // it when the local error is eating the whole budget (the
                    // higher-order method is not earning its keep on this
                    // problem / step size).
                    if ns.order < BDF_MAX_ORDER && err_est < 0.6 {
                        bdf_steps_at_order += 1;
                        if bdf_steps_at_order >= 3 {
                            ns.increase_order();
                            bdf_steps_at_order = 0;
                        }
                    } else if ns.order > 1 && err_est > 0.95 {
                        ns.decrease_order();
                        bdf_steps_at_order = 0;
                    } else {
                        bdf_steps_at_order = 0;
                    }
                    // Rescale the Nordsieck vector to the next step size.
                    if (ns.h - next).abs() > 1e-14 * next.abs() {
                        ns.rescale(next);
                    }
                }

                t = res.t;
                y = res.y;
                f_cur = res.f_new;
                h = next;
                steps += 1;
                if steps > max_steps {
                    return Err(OdeError::MaxSteps { t_final, max_steps });
                }
                if dir * (t_final - t) <= 0.0 {
                    record_final(t, &y, t_eval, &mut eval_idx, t_final, dir, &mut outputs);
                    break;
                }
            }
            Err(OdeError::Newton { t: _, residual: _ }) | Err(OdeError::StepTooSmall { t: _ }) => {
                let contracted = h * 0.5;
                if contracted.abs() < h_min {
                    return Err(OdeError::StepTooSmall { t });
                }
                h = contracted;
            }
            Err(e) => return Err(e),
        }
    }

    Ok(Integration {
        outputs,
        t_end: t,
        y_end: y,
        hits,
    })
}

/// A terminal stop: crossing time, state there, and derivative there.
type StopState = (f64, Vec<f64>, Vec<f64>);

/// Check every event for a sign change across the accepted step `t0 → res.t`.
///
/// Each crossing is located by bisection on the step's Hermite interpolant and
/// appended to `hits` in time order. If a terminal event fires, returns the
/// crossing time, state and derivative so the caller can stop there; later
/// crossings in the same step are discarded.
fn detect_events(
    events: &[Event],
    f: &RhsFn<'_>,
    t0: f64,
    y0: &[f64],
    f0: &[f64],
    res: &StepResult,
    hits: &mut Vec<EventHit>,
) -> Result<Option<StopState>, OdeError> {
    if events.is_empty() {
        return Ok(None);
    }
    let interp = |tau: f64| hermite(t0, y0, f0, res.t, &res.y, &res.f_new, tau);

    let mut found: Vec<(f64, usize, Direction)> = Vec::new();
    for (idx, ev) in events.iter().enumerate() {
        let g0 = ev.eval(t0, y0);
        let g1 = ev.eval(res.t, &res.y);
        if let Some(dir) = ev.crossing(g0, g1) {
            let te = bisect_root(t0, g0, res.t, |tau| ev.eval(tau, &interp(tau)));
            found.push((te, idx, dir));
        }
    }
    // Sort by distance travelled from t0, so simultaneous crossings keep time order.
    found.sort_by(|a, b| (a.0 - t0).abs().total_cmp(&(b.0 - t0).abs()));

    for (te, idx, dir) in found {
        let y_te = interp(te);
        hits.push(EventHit {
            index: idx,
            t: te,
            y: y_te.clone(),
            direction: dir,
        });
        if events[idx].is_terminal() {
            let f_te = eval(f, te, &y_te)?;
            return Ok(Some((te, y_te, f_te)));
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn record_outputs(
    t_old: f64,
    y_old: &[f64],
    f_old: &[f64],
    res: &StepResult,
    t_eval: Option<&[f64]>,
    eval_idx: &mut usize,
    dir: f64,
    outputs: &mut Vec<Vec<f64>>,
) {
    if let Some(te) = t_eval {
        while *eval_idx < te.len() && dir * (te[*eval_idx] - t_old) <= dir * (res.t - t_old) {
            let tev = te[*eval_idx];
            outputs.push(hermite(t_old, y_old, f_old, res.t, &res.y, &res.f_new, tev));
            *eval_idx += 1;
        }
    }
}

fn record_final(
    t: f64,
    y: &[f64],
    t_eval: Option<&[f64]>,
    eval_idx: &mut usize,
    t_final: f64,
    dir: f64,
    outputs: &mut Vec<Vec<f64>>,
) {
    if let Some(te) = t_eval {
        while *eval_idx < te.len() && dir * (te[*eval_idx] - t) <= 0.0 {
            outputs.push(y.to_vec());
            *eval_idx += 1;
        }
        if *eval_idx < te.len() && (te[*eval_idx] - t_final).abs() < 1e-12 {
            outputs.push(y.to_vec());
            *eval_idx += 1;
        }
    } else {
        let _ = (t, t_final);
    }
}

/// Cubic Hermite interpolation at `tau ∈ [t0, t1]`.
fn hermite(t0: f64, y0: &[f64], f0: &[f64], t1: f64, y1: &[f64], f1: &[f64], tau: f64) -> Vec<f64> {
    let h = t1 - t0;
    let s = (tau - t0) / h;
    let s2 = s * s;
    let s3 = s2 * s;
    let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
    let h10 = s3 - 2.0 * s2 + s;
    let h01 = -2.0 * s3 + 3.0 * s2;
    let h11 = s3 - s2;
    let mut out = vec![0.0; y0.len()];
    for i in 0..y0.len() {
        out[i] = h00 * y0[i] + h10 * h * f0[i] + h01 * y1[i] + h11 * h * f1[i];
    }
    out
}

fn weighted_norm(err: &[f64], y: &[f64], rtol: f64, atol: f64) -> f64 {
    let n = err.len();
    let mut sum = 0.0;
    for i in 0..n {
        let w = atol + rtol * y[i].abs();
        sum += (err[i] / w).powi(2);
    }
    (sum / n as f64).sqrt()
}

// ---------------------------------------------------------------------------
// Dormand–Prince RK5(4) — explicit, non-stiff.
// ---------------------------------------------------------------------------

const DP_C: [f64; 7] = [0.0, 1.0 / 5.0, 3.0 / 10.0, 4.0 / 5.0, 8.0 / 9.0, 1.0, 1.0];
// Lower-triangular A (row-indexed by stage s, columns 0..s).
const DP_A: [[f64; 6]; 6] = [
    [1.0 / 5.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [3.0 / 40.0, 9.0 / 40.0, 0.0, 0.0, 0.0, 0.0],
    [44.0 / 45.0, -56.0 / 15.0, 32.0 / 9.0, 0.0, 0.0, 0.0],
    [
        19372.0 / 6561.0,
        -25360.0 / 2187.0,
        64448.0 / 6561.0,
        -212.0 / 729.0,
        0.0,
        0.0,
    ],
    [
        9017.0 / 3168.0,
        -355.0 / 33.0,
        46732.0 / 5247.0,
        49.0 / 176.0,
        -5103.0 / 18656.0,
        0.0,
    ],
    [
        35.0 / 384.0,
        0.0,
        500.0 / 1113.0,
        125.0 / 192.0,
        -2187.0 / 6784.0,
        11.0 / 84.0,
    ],
];
const DP_B: [f64; 7] = [
    35.0 / 384.0,
    0.0,
    500.0 / 1113.0,
    125.0 / 192.0,
    -2187.0 / 6784.0,
    11.0 / 84.0,
    0.0,
];
const DP_BHAT: [f64; 7] = [
    5179.0 / 57600.0,
    0.0,
    7571.0 / 16695.0,
    393.0 / 640.0,
    -92097.0 / 339200.0,
    187.0 / 2100.0,
    1.0 / 40.0,
];

fn step_dp54(
    f: &dyn crate::RhsCallable,
    t: f64,
    y: &[f64],
    h: f64,
) -> Result<StepResult, OdeError> {
    let n = y.len();
    let mut k = vec![vec![0.0; n]; 7];
    k[0] = eval(f, t, y)?;
    for s in 1..7 {
        let mut stage = vec![0.0; n];
        for i in 0..n {
            let mut acc = y[i];
            for col in 0..s {
                let a = DP_A[s - 1][col];
                if a != 0.0 {
                    acc += h * a * k[col][i];
                }
            }
            stage[i] = acc;
        }
        k[s] = eval(f, t + h * DP_C[s], &stage)?;
    }
    let mut y_new = vec![0.0; n];
    let mut y_hat = vec![0.0; n];
    for i in 0..n {
        let mut acc5 = y[i];
        let mut acc4 = y[i];
        for s in 0..7 {
            acc5 += h * DP_B[s] * k[s][i];
            acc4 += h * DP_BHAT[s] * k[s][i];
        }
        y_new[i] = acc5;
        y_hat[i] = acc4;
    }
    let err = y_new
        .iter()
        .zip(&y_hat)
        .map(|(a, b)| a - b)
        .collect::<Vec<_>>();
    let f_new = eval(f, t + h, &y_new)?;
    Ok(StepResult {
        t: t + h,
        y: y_new,
        err,
        f_new,
    })
}

// ---------------------------------------------------------------------------
// TR-BDF2 — 3-stage ESDIRK form (explicit first row), A-/L-stable, stiff.
// Bank et al. (1985). γ = 2 - √2 ≈ 0.585786 is the split point; d = γ/2.
//
// Stage 1 (trapezoid over [t, t+γh]): y_s = y + d·h·(k0 + k1), k1 = f(t+γh, y_s).
// Stage 2 (BDF2 over [t, t+h]):       y₊ = y + h·(w0·k0 + w1·k1 + w2·k2),
//                                     k2 = f(t+h, y₊).
// Consistency: row sums equal the abscissae (γ and 1) and the weights satisfy
// Σw = 1 and w·c = 1/2, so the scheme is second order.
// ---------------------------------------------------------------------------

const TRBDF2_GAMMA: f64 = 0.585_786_437_626_904_9; // 2 - sqrt(2)
const TRBDF2_D: f64 = TRBDF2_GAMMA / 2.0;
const TRBDF2_W0: f64 = 0.353_553_390_593_273_73; // 1 / (2(2 - γ)) = 1/(2√2)
const TRBDF2_W1: f64 = TRBDF2_W0;
const TRBDF2_W2: f64 = 0.292_893_218_813_452_5; // (1 - γ) / (2 - γ)

fn step_sdirk2(
    f: &dyn crate::RhsCallable,
    t: f64,
    y: &[f64],
    h: f64,
    tol: Tol,
    jac: Option<&JacobianPattern>,
) -> Result<StepResult, OdeError> {
    let n = y.len();
    let g = TRBDF2_GAMMA;
    let d = TRBDF2_D;
    let f_start = eval(f, t, y)?;

    // Stage 1: k1 = f(t + γh, y + d·h·k0 + d·h·k1) — an SDIRK solve with the
    // explicit part of the trapezoid folded into the base state.
    let base1: Vec<f64> = y
        .iter()
        .zip(&f_start)
        .map(|(yi, k0i)| yi + d * h * k0i)
        .collect();
    let k1 = sdirk_stage(f, t + g * h, &base1, d * h, &f_start, tol, jac)?;

    // Stage 2: y₊ = y + h(w0·k0 + w1·k1) + h·w2·k2, k2 = f(t + h, y₊).
    let base2: Vec<f64> = (0..n)
        .map(|i| y[i] + h * (TRBDF2_W0 * f_start[i] + TRBDF2_W1 * k1[i]))
        .collect();
    let k2 = sdirk_stage(f, t + h, &base2, TRBDF2_W2 * h, &k1, tol, jac)?;

    let y_new: Vec<f64> = (0..n).map(|i| base2[i] + TRBDF2_W2 * h * k2[i]).collect();
    // k2 = f(t + h, y_new) by construction, so the derivative at the end of the
    // step is already available without another RHS evaluation.
    let f_new = k2;

    // Embedded error: the difference between the TR-BDF2 solution and the full-step
    // trapezoidal solution y + h/2·(f(t, y) + f(t + h, y_new)). Both are second
    // order, so their difference is O(h³) locally, which matches the controller
    // exponent in `Method::error_order`.
    let err: Vec<f64> = (0..n)
        .map(|i| y_new[i] - (y[i] + 0.5 * h * (f_start[i] + f_new[i])))
        .collect();
    Ok(StepResult {
        t: t + h,
        y: y_new,
        err,
        f_new,
    })
}

// ---------------------------------------------------------------------------
// ESDIRK34 — 4-stage ESDIRK order 3(4), A-/L-stable, stiff
// (Kennedy & Carpenter 2003, with B weights normalized to sum to 1).
// ---------------------------------------------------------------------------

const ESDIRK34_GAMMA: f64 = 0.435_866_521_508_459;
const ESDIRK34_C2: f64 = 0.871_733_043_016_918; // 2·gamma
const ESDIRK34_C3: f64 = 0.468_238_744_851_844_4;
const ESDIRK34_A31: f64 = 0.140_737_774_724_706_2;
const ESDIRK34_A32: f64 = -0.108_365_551_381_320_8;
// B weights (order 3, stiffly accurate: the last weight is γ, which equals the
// final stage's diagonal, so y_{n+1} = Y_4). Solved from the order-1..3 conditions
// Σb = 1, Σb·c = 1/2, Σb·c² = 1/3, Σ_i b_i Σ_j a_ij c_j = 1/6 with the A-row
// coefficients above; the residual of every condition is ~1e-17.
const ESDIRK34_B: [f64; 4] = [
    0.102_399_400_616_089,
    -0.376_878_452_267_324,
    0.838_612_530_188_334,
    ESDIRK34_GAMMA,
];
// Embedded weights (order 4): sum to 1.
const ESDIRK34_BHAT: [f64; 4] = [
    0.157_024_897_860_324_95,
    0.117_330_441_370_438_85,
    0.616_678_030_392_121_4,
    0.108_966_630_377_114_75,
];

fn step_esdirk34(
    f: &dyn crate::RhsCallable,
    t: f64,
    y: &[f64],
    h: f64,
    tol: Tol,
    jac: Option<&JacobianPattern>,
) -> Result<StepResult, OdeError> {
    let n = y.len();
    let g = ESDIRK34_GAMMA;
    let k0 = eval(f, t, y)?; // explicit first stage

    // Stage 1 (c = 2g): Y1 = y + h·(g·k0 + g·k1).
    let base1: Vec<f64> = y
        .iter()
        .zip(&k0)
        .map(|(yi, k0i)| yi + h * g * k0i)
        .collect();
    let k1 = sdirk_stage(f, t + ESDIRK34_C2 * h, &base1, g * h, &k0, tol, jac)?;

    // Stage 2 (c = c3): Y2 = y + h·(a31·k0 + a32·k1 + g·k2).
    let base2: Vec<f64> = y
        .iter()
        .zip(&k0)
        .zip(&k1)
        .map(|((yi, k0i), k1i)| yi + h * (ESDIRK34_A31 * k0i + ESDIRK34_A32 * k1i))
        .collect();
    let k2 = sdirk_stage(f, t + ESDIRK34_C3 * h, &base2, g * h, &k1, tol, jac)?;

    // Stage 3 (c = 1): Y3 = y + h·(b0·k0 + b1·k1 + b2·k2 + g·k3).
    let base3: Vec<f64> = y
        .iter()
        .zip(&k0)
        .zip(&k1)
        .zip(&k2)
        .map(|(((yi, k0i), k1i), k2i)| {
            yi + h * (ESDIRK34_B[0] * k0i + ESDIRK34_B[1] * k1i + ESDIRK34_B[2] * k2i)
        })
        .collect();
    let k3 = sdirk_stage(f, t + h, &base3, g * h, &k2, tol, jac)?;

    let ks = [&k0, &k1, &k2, &k3];
    let mut y_new = y.to_vec();
    let mut y_hat = y.to_vec();
    for i in 0..n {
        let mut s5 = y[i];
        let mut s4 = y[i];
        for j in 0..4 {
            s5 += h * ESDIRK34_B[j] * ks[j][i];
            s4 += h * ESDIRK34_BHAT[j] * ks[j][i];
        }
        y_new[i] = s5;
        y_hat[i] = s4;
    }
    let err = y_new
        .iter()
        .zip(&y_hat)
        .map(|(a, b)| a - b)
        .collect::<Vec<_>>();
    let f_new = eval(f, t + h, &y_new)?;
    Ok(StepResult {
        t: t + h,
        y: y_new,
        err,
        f_new,
    })
}

// ---------------------------------------------------------------------------
// BDF — variable-order (1–5) backward differentiation, stiff.
//
// Implemented in the Nordsieck (Gear) representation. The Nordsieck vector is
// stored column-major as `z[col][comp]` for `col = 0..=MAX_ORDER`:
//   z[0] = y,  z[k] = h^k/k! · y^(k)   (the scaled k-th derivative at the
//   current time, for k ≥ 1).
//
// Each step uses the fixed-leading-coefficient (FLC) BDF corrector (the form
// used by SUNDIALS/CVODE):
//   y_c - y_p = h·β0·(f(t, y_c) - ẏ_p),   ẏ_p = z[1]/h,
// where y_p = Σ_k z[k] is the Taylor predictor and β0 = 1/(1 + 1/2 + … + 1/q).
// After convergence the vector is corrected with the standard Nordsieck
// coefficients l_j (derived from the corrector polynomial c(t)), and the local
// truncation error is estimated from the change in the highest column.
// ---------------------------------------------------------------------------

const BDF_MAX_ORDER: usize = 5;

// β0 = 1/(1 + 1/2 + … + 1/q) for q = 1..5.
const BDF_BETA0: [f64; 5] = [1.0, 2.0 / 3.0, 6.0 / 11.0, 12.0 / 25.0, 60.0 / 137.0];

// Nordsieck corrector coefficients l_j for orders q = 1..5.
// l_j = g^{(j)}(0) / (j!·q!)  with  g(u) = ∏_{i=1}^q (u + i).
const BDF_L: [[f64; 6]; 5] = [
    [1.0, 1.0, 0.0, 0.0, 0.0, 0.0],
    [1.0, 1.5, 0.5, 0.0, 0.0, 0.0],
    [1.0, 11.0 / 6.0, 1.0, 1.0 / 6.0, 0.0, 0.0],
    [1.0, 25.0 / 12.0, 35.0 / 24.0, 5.0 / 12.0, 1.0 / 24.0, 0.0],
    [
        1.0,
        137.0 / 60.0,
        15.0 / 8.0,
        17.0 / 24.0,
        1.0 / 8.0,
        1.0 / 120.0,
    ],
];

// Delta-to-local-error scaling factors r_q = C_{q+1} / (C_{q+1} + 1/(q+1)!)
// for orders q = 1..5, used to turn the corrector correction delta into an
// absolute local-error estimate.
const BDF_R: [f64; 5] = [0.5, 5.0 / 7.0, 0.9, 251.0 / 257.0, 238.0 / 239.0];

/// Binomial coefficient C(n, k) for small n (n ≤ 10).
fn binom(n: usize, k: usize) -> f64 {
    if k > n {
        return 0.0;
    }
    if k == 0 || k == n {
        return 1.0;
    }
    let mut res = 1.0;
    for i in 0..k {
        res = res * (n - i) as f64 / (i + 1) as f64;
    }
    res
}

/// Nordsieck vector state for the variable-order (1–5) BDF.
#[derive(Clone)]
struct NordsieckState {
    /// `z[col][comp]`: column-major Nordsieck array, `col = 0..=BDF_MAX_ORDER`.
    z: Vec<Vec<f64>>,
    /// Current method order (1–5).
    order: usize,
    /// Current step size (the array is scaled to this h).
    h: f64,
}

impl NordsieckState {
    fn new(n: usize) -> Self {
        let z = vec![vec![0.0; n]; BDF_MAX_ORDER + 1];
        NordsieckState {
            z,
            order: 1,
            h: 0.0,
        }
    }

    fn n(&self) -> usize {
        self.z[0].len()
    }

    /// Seed the state at `t0` from the initial `y0`/`f0` and first step `h`.
    fn initialize(&mut self, y0: &[f64], f0: &[f64], h: f64) {
        self.order = 1;
        self.h = h;
        let n = y0.len();
        for i in 0..n {
            self.z[0][i] = y0[i];
            self.z[1][i] = h * f0[i];
            for c in 2..=BDF_MAX_ORDER {
                self.z[c][i] = 0.0;
            }
        }
    }

    /// Rescale the Nordsieck columns for a step-size change `h_new`
    /// (column `c` scales by `(h_new/h)^c`).
    fn rescale(&mut self, h_new: f64) {
        if self.h == 0.0 {
            self.h = h_new;
            return;
        }
        let ratio = h_new / self.h;
        if (ratio - 1.0).abs() < 1e-14 {
            return;
        }
        let n = self.n();
        for c in 0..=BDF_MAX_ORDER {
            let s = ratio.powi(c as i32);
            for i in 0..n {
                self.z[c][i] *= s;
            }
        }
        self.h = h_new;
    }

    /// Predictor: binomial shift of the stored array to the next point,
    /// `z_pred[j] = Σ_{m=0}^{order-j} C(j+m, j)·z[j+m]`.
    #[allow(clippy::needless_range_loop)]
    fn predict(&self) -> Vec<Vec<f64>> {
        let n = self.n();
        let order = self.order;
        let mut zp = vec![vec![0.0; n]; BDF_MAX_ORDER + 1];
        for j in 0..=order {
            for i in 0..n {
                let mut s = 0.0;
                for m in 0..=(order - j) {
                    s += binom(j + m, j) * self.z[j + m][i];
                }
                zp[j][i] = s;
            }
        }
        zp
    }

    fn increase_order(&mut self) {
        if self.order < BDF_MAX_ORDER {
            self.order += 1;
        }
    }

    fn decrease_order(&mut self) {
        if self.order > 1 {
            self.order -= 1;
        }
    }
}

#[allow(clippy::needless_range_loop)]
fn step_bdf(
    f: &dyn crate::RhsCallable,
    t: f64,
    _y: &[f64],
    h: f64,
    tol: Tol,
    jac: Option<&JacobianPattern>,
    state: &mut NordsieckState,
) -> Result<StepResult, OdeError> {
    let n = state.n();

    // Rescale if the requested step differs from the stored one (e.g. a
    // rejected step shrank h, or the driver advanced h for the next step).
    if (state.h - h).abs() > 1e-14 * h.abs() {
        state.rescale(h);
    }

    let order = state.order;
    let beta0 = BDF_BETA0[order - 1];
    let gamma = h * beta0;

    let z_pred = state.predict();
    let y_p = &z_pred[0];
    // Predicted derivative from the Nordsieck array: ẏ_p = z[1]/h.
    let ydot_p: Vec<f64> = z_pred[1].iter().map(|v| v / h).collect();

    // Highest column at the start of the step, for the LTE estimate.
    let last_col_prev = state.z[order].clone();

    // Newton corrector for the FLC-BDF equation
    //   y_c - y_p - γ·(f(t+h, y_c) - ẏ_p) = 0,  γ = h·β0.
    // Convergence is tested on the weighted norm of the Newton correction, so
    // the test scales with the tolerances rather than with absolute magnitudes.
    let mut y_cur = y_p.clone();
    let mut fk = eval(f, t + h, &y_cur)?;
    let mut corr = f64::INFINITY;
    let mut converged = false;
    for _iter in 0..30 {
        let residual: Vec<f64> = (0..n)
            .map(|i| y_cur[i] - y_p[i] - gamma * (fk[i] - ydot_p[i]))
            .collect();
        let rnorm = residual.iter().map(|r| r * r).sum::<f64>().sqrt();
        // Large systems route the Newton linear solve through the sparse
        // CSR/LU path (`sparse` module): the finite-difference Jacobian is
        // built directly in compressed storage and never materialised densely.
        let delta = if n >= crate::sparse::SPARSE_LU_MIN_N {
            let jmat = match jac {
                Some(pattern) => crate::sparse::CsrMatrix::jacobian_colored(
                    |t, y, dydt| f.call(t, y, dydt),
                    t + h,
                    &y_cur,
                    &fk,
                    pattern,
                )?,
                None => crate::sparse::CsrMatrix::jacobian(
                    |t, y, dydt| f.call(t, y, dydt),
                    t + h,
                    &y_cur,
                    &fk,
                    None,
                )?,
            };
            let a = jmat.scaled_identity_minus_scaled(gamma);
            a.sparse_solve(&residual).ok_or(OdeError::Newton {
                t: t + h,
                residual: rnorm,
            })?
        } else {
            let jac = jacobian(f, t + h, &y_cur, &fk)?;
            let mut a = DMat::new(n, n);
            for i in 0..n {
                for j in 0..n {
                    a.set(i, j, if i == j { 1.0 } else { 0.0 } - gamma * jac.get(i, j));
                }
            }
            a.solve(&residual).ok_or(OdeError::Newton {
                t: t + h,
                residual: rnorm,
            })?
        };
        for i in 0..n {
            y_cur[i] -= delta[i];
        }
        corr = tol.wrms(&delta, &y_cur);
        fk = eval(f, t + h, &y_cur)?;
        if corr < NEWTON_TOL {
            converged = true;
            break;
        }
    }
    if !converged {
        return Err(OdeError::Newton {
            t: t + h,
            residual: corr,
        });
    }
    let f_new = fk;

    // Correction relative to the predicted solution.
    let delta: Vec<f64> = y_cur.iter().zip(y_p).map(|(a, b)| a - b).collect();

    // Nordsieck corrector update:  z_new[j] = z_pred[j] + l_j·δ.
    let l = &BDF_L[order - 1];
    for j in 0..=order {
        for i in 0..n {
            state.z[j][i] = z_pred[j][i] + l[j] * delta[i];
        }
    }

    // Local truncation error from the correction delta. The BDF corrector of
    // order q satisfies (predictor error) = (corrector error) + delta, with
    // predictor error = h^(q+1)/(q+1)!*y^(q+1) and corrector (local) error
    // LTE = C_{q+1}*h^(q+1)*y^(q+1). Hence LTE = r_q*delta, where
    //   r_q = C_{q+1} / (C_{q+1} + 1/(q+1)!).
    // This estimate is reliable even when the higher Nordsieck columns are
    // still being populated after an order change.
    let r = BDF_R[order - 1];
    let mut err = vec![0.0; n];
    for i in 0..n {
        err[i] = r * delta[i];
    }

    // Proactively bootstrap the NEXT Nordsieck column from the within-step
    // finite difference of the current highest column (at a single, consistent
    // step size), so it is ready when the order is later raised.
    if order < BDF_MAX_ORDER {
        let denom = (order as f64) + 1.0;
        for i in 0..n {
            state.z[order + 1][i] = (state.z[order][i] - last_col_prev[i]) / denom;
        }
    }

    Ok(StepResult {
        t: t + h,
        y: y_cur,
        err,
        f_new,
    })
}

impl OdeProblem {
    /// Integrate the problem from its initial time to `t_final` and return the
    /// state vector at `t_final`, using the requested `method`.
    ///
    /// # Errors
    ///
    /// Returns [`OdeError`] if the integration fails (non-convergent Newton
    /// step, collapsed step size, or step budget exceeded).
    pub fn solve(&self, method: Method, t_final: f64) -> Result<Vec<f64>, OdeError> {
        Ok(integrate(self, method, t_final, None, &[])?.y_end)
    }

    /// Integrate to `t_final` while watching the given [`Event`]s.
    ///
    /// Every sign change of an event function is reported in
    /// [`EventSolution::hits`]. Each crossing is bracketed by the accepted step
    /// and located by bisection on that step's cubic-Hermite dense output, so its
    /// accuracy is that of the interpolant (roughly `O(h⁴)` per step), not
    /// round-off. If a terminal event fires, integration stops at its
    /// crossing and [`EventSolution::t_end`] is that time; otherwise `t_end` is
    /// `t_final`.
    ///
    /// # Errors
    ///
    /// Returns [`OdeError`] if the integration fails (see [`OdeProblem::solve`]).
    ///
    /// # Examples
    ///
    /// A bouncing ball that hits the floor at `t = √(2h/g)`:
    ///
    /// ```
    /// use tpt_sci_ode::{Method, OdeProblem};
    /// use tpt_sci_ode::events::{Direction, Event};
    ///
    /// // State [height, velocity]; g = 9.81.
    /// let prob = OdeProblem::new(
    ///     |_t, y, dy| {
    ///         dy[0] = y[1];
    ///         dy[1] = -9.81;
    ///     },
    ///     vec![10.0, 0.0],
    ///     0.0,
    /// )
    /// .unwrap();
    /// let floor = Event::new(|_t, y| y[0])
    ///     .direction(Direction::Falling)
    ///     .terminal();
    /// let sol = prob.solve_events(Method::Tsit45, 10.0, &[floor]).unwrap();
    /// let expected = (2.0 * 10.0 / 9.81f64).sqrt();
    /// assert!((sol.t_end - expected).abs() < 1e-6);
    /// ```
    pub fn solve_events(
        &self,
        method: Method,
        t_final: f64,
        events: &[Event],
    ) -> Result<EventSolution, OdeError> {
        let run = integrate(self, method, t_final, None, events)?;
        Ok(EventSolution {
            t_end: run.t_end,
            y_end: run.y_end,
            hits: run.hits,
        })
    }

    /// Integrate at each time in `t_eval` (each strictly increasing and beyond
    /// `t0`) and return one state vector per evaluation time.
    ///
    /// The integrator lands exactly on each `t_eval` point and interpolates with
    /// a cubic Hermite polynomial between accepted steps, so output is exact at
    /// the requested times.
    ///
    /// # Errors
    ///
    /// Returns [`OdeError`] if the integration fails (see [`OdeProblem::solve`]).
    ///
    /// # Panics
    ///
    /// Panics if `t_eval` is non-empty but its last element cannot be read
    /// (only reachable if `t_eval` is mutated concurrently); for a normal
    /// non-empty slice this is unreachable.
    pub fn solve_dense(&self, method: Method, t_eval: &[f64]) -> Result<Vec<Vec<f64>>, OdeError> {
        if t_eval.is_empty() {
            return Ok(Vec::new());
        }
        Ok(integrate(self, method, *t_eval.last().unwrap(), Some(t_eval), &[])?.outputs)
    }
}
