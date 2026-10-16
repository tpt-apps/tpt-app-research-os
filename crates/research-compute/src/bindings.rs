//! Host functions a cell can call, built on `tpt-math`.
//!
//! Every function here is deterministic. Random draws come from the session seed,
//! so a document run twice with the same seed gives the same numbers. Use decimal
//! literals (`2.0`, not `2`) for float arguments.

use std::cell::RefCell;
use std::rc::Rc;

use research_doc::Output;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Position};
use tpt_math_linalg_dense::{DMatrix, DVector};
use tpt_math_prob_dist::{normal, Dist, Distribution, SplitMix64};

use crate::engine::{splitmix64, uniform_at, Limits};
use crate::ode::ode_solve;

/// Register every binding on `engine`, for a session with `seed`.
pub(crate) fn register(
    engine: &mut Engine,
    seed: u64,
    limits: Limits,
    shown: Rc<RefCell<Vec<Output>>>,
) {
    engine.register_fn("uniform", move |index: i64| uniform_at(seed, index));

    engine.register_fn(
        "normal",
        move |index: i64, mean: f64, sd: f64| -> Result<f64, Box<EvalAltResult>> {
            let distribution = normal((mean, sd)).map_err(runtime_text)?;
            // A separate stream from `uniform`, so the two never share draws.
            let mut rng = SplitMix64::seed_from_u64(splitmix64(
                seed ^ splitmix64(index as u64 ^ NORMAL_STREAM),
            ));
            Ok(Dist::new(distribution).sample(&mut rng))
        },
    );

    // Cloned before `ode_solve` takes `limits`, for the two `diff` overloads below.
    let (diff_limits, diff_limits_with_params) = (limits.clone(), limits.clone());
    engine.register_fn(
        "ode_solve",
        move |rhs: String,
              params: Map,
              y0: Array,
              t0: f64,
              t_eval: Array,
              method: String|
              -> Result<Array, Box<EvalAltResult>> {
            ode_solve(rhs, params, y0, t0, t_eval, method, &limits)
        },
    );

    engine.register_fn(
        "diff",
        move |expr: String, x: f64| -> Result<f64, Box<EvalAltResult>> {
            crate::autodiff::diff(expr, x, Map::new(), &diff_limits)
        },
    );
    engine.register_fn(
        "diff",
        move |expr: String, x: f64, params: Map| -> Result<f64, Box<EvalAltResult>> {
            crate::autodiff::diff(expr, x, params, &diff_limits_with_params)
        },
    );

    let plots = shown.clone();
    engine.register_fn(
        "show_plot",
        move |svg: String| -> Result<(), Box<EvalAltResult>> {
            if !svg.trim_start().starts_with("<svg") {
                return Err(runtime_text(
                    "show_plot needs SVG text that starts with <svg",
                ));
            }
            plots.borrow_mut().push(Output::Svg { svg });
            Ok(())
        },
    );

    engine.register_fn(
        "show_table",
        move |columns: Array, rows: Array| -> Result<(), Box<EvalAltResult>> {
            let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
            if columns.is_empty() {
                return Err(runtime_text("a table needs at least one column"));
            }
            let mut body = Vec::with_capacity(rows.len());
            for row in rows {
                let row = row
                    .into_array()
                    .map_err(|_| runtime_text("each table row must be an array"))?;
                if row.len() != columns.len() {
                    return Err(runtime_text("a table row does not match the column count"));
                }
                body.push(row.iter().map(|v| v.to_string()).collect());
            }
            shown.borrow_mut().push(Output::Table {
                columns,
                rows: body,
            });
            Ok(())
        },
    );

    engine.register_fn(
        "solve",
        |a: Array, b: Array| -> Result<Array, Box<EvalAltResult>> { solve(a, b) },
    );
}

const NORMAL_STREAM: u64 = 0xA5A5_A5A5_5A5A_5A5A;

/// Solve the square linear system `a * x = b` and return `x`.
fn solve(a: Array, b: Array) -> Result<Array, Box<EvalAltResult>> {
    let n = a.len();
    let mut flat = Vec::with_capacity(n * n);
    for row in a {
        let row = row
            .into_array()
            .map_err(|_| runtime_text("a must be an array of rows"))?;
        if row.len() != n {
            return Err(runtime_text("a must be square"));
        }
        for cell in row {
            flat.push(as_float(&cell).ok_or_else(|| runtime_text("a must hold numbers"))?);
        }
    }
    let rhs = b
        .iter()
        .map(|v| as_float(v).ok_or_else(|| runtime_text("b must hold numbers")))
        .collect::<Result<Vec<f64>, _>>()?;
    if rhs.len() != n {
        return Err(runtime_text("b must have one entry per row of a"));
    }

    let matrix = DMatrix::from_row_slice(n, n, &flat);
    let x = matrix
        .solve(&DVector::from_vec(rhs))
        .map_err(|e| runtime_text(&e.to_string()))?;
    Ok(x.as_slice().iter().map(|v| Dynamic::from(*v)).collect())
}

fn as_float(value: &Dynamic) -> Option<f64> {
    value
        .as_float()
        .ok()
        .or_else(|| value.as_int().ok().map(|i| i as f64))
}

fn runtime_text(message: &str) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(message.to_string()),
        Position::NONE,
    ))
}
