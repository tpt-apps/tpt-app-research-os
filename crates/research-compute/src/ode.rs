//! `ode_solve`: integrate an initial value problem with `tpt-sci-ode`.
//!
//! The right-hand side is an expression string over `t` (time), `y` (state
//! array) and the entries of the parameter map. It must evaluate to an array
//! with one derivative per state. The expression is compiled once per call, and
//! each evaluation runs under the same limits as a cell.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Position, Scope, AST};
use tpt_sci_ode::{Method, OdeProblem};

use crate::engine::Limits;

/// Solve `y' = rhs(t, y)` from `t_eval[0]` with initial state `y0`, and return the
/// state at each time in `t_eval`.
pub(crate) fn ode_solve(
    rhs: String,
    params: Map,
    y0: Array,
    t0: f64,
    t_eval: Array,
    method: String,
    limits: &Limits,
) -> Result<Array, Box<EvalAltResult>> {
    let method = parse_method(&method)?;
    let y0 = floats(y0, "y0")?;
    let t_eval = floats(t_eval, "t_eval")?;
    if t_eval.is_empty() {
        return Err(runtime("t_eval must list at least one time after t0"));
    }
    let params = parameters(params)?;

    let mut engine = Engine::new();
    engine.set_max_operations(limits.max_operations);
    engine.set_max_string_size(limits.max_string_bytes);
    engine.set_max_array_size(limits.max_collection_len);
    engine.set_max_call_levels(limits.max_call_depth);
    let ast: AST = engine
        .compile_expression(&rhs)
        .map_err(|e| runtime(&format!("rhs does not compile: {e}")))?;
    let engine = Rc::new(engine);
    let ast = Rc::new(ast);

    let failure: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let params = Rc::new(params);
    let rhs_failure = failure.clone();
    let problem = OdeProblem::new(
        move |t: f64, y: &[f64], dydt: &mut [f64]| {
            if rhs_failure.borrow().is_some() {
                dydt.fill(0.0);
                return;
            }
            let mut scope = Scope::new();
            scope.push("t", t);
            scope.push("y", y.iter().map(|v| Dynamic::from(*v)).collect::<Array>());
            for (name, value) in params.iter() {
                scope.push(name.clone(), *value);
            }
            match engine.eval_ast_with_scope::<Array>(&mut scope, &ast) {
                Ok(values) if values.len() == dydt.len() => {
                    for (slot, value) in dydt.iter_mut().zip(values) {
                        *slot = value
                            .as_float()
                            .ok()
                            .or_else(|| value.as_int().ok().map(|i| i as f64))
                            .unwrap_or(f64::NAN);
                    }
                }
                Ok(values) => {
                    *rhs_failure.borrow_mut() = Some(format!(
                        "rhs returned {} values for {} states",
                        values.len(),
                        dydt.len()
                    ));
                    dydt.fill(0.0);
                }
                Err(e) => {
                    *rhs_failure.borrow_mut() = Some(e.to_string());
                    dydt.fill(0.0);
                }
            }
        },
        y0,
        t0,
    )
    .map_err(|e| runtime(&e.to_string()))?;

    let states = match problem.solve_dense(method, &t_eval) {
        Ok(states) => states,
        Err(e) => {
            let detail = failure.borrow().clone();
            return Err(runtime(&detail.unwrap_or_else(|| e.to_string())));
        }
    };
    if let Some(detail) = failure.borrow().clone() {
        return Err(runtime(&detail));
    }
    Ok(states
        .into_iter()
        .map(|state| {
            state
                .into_iter()
                .map(Dynamic::from)
                .collect::<Array>()
                .into()
        })
        .collect())
}

fn parse_method(name: &str) -> Result<Method, Box<EvalAltResult>> {
    match name {
        "bdf" => Ok(Method::Bdf),
        "tsit45" => Ok(Method::Tsit45),
        "trbdf2" => Ok(Method::TrBdf2),
        "esdirk34" => Ok(Method::Esdirk34),
        other => Err(runtime(&format!(
            "unknown method {other:?}; use bdf, tsit45, trbdf2 or esdirk34"
        ))),
    }
}

fn floats(values: Array, what: &str) -> Result<Vec<f64>, Box<EvalAltResult>> {
    values
        .iter()
        .map(|v| {
            v.as_float()
                .ok()
                .or_else(|| v.as_int().ok().map(|i| i as f64))
                .ok_or_else(|| runtime(&format!("{what} must hold numbers")))
        })
        .collect()
}

fn parameters(params: Map) -> Result<BTreeMap<String, f64>, Box<EvalAltResult>> {
    params
        .into_iter()
        .map(|(name, value)| {
            let number = value
                .as_float()
                .ok()
                .or_else(|| value.as_int().ok().map(|i| i as f64))
                .ok_or_else(|| runtime(&format!("parameter {name} must be a number")))?;
            Ok((name.to_string(), number))
        })
        .collect()
}

fn runtime(message: &str) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(message.to_string()),
        Position::NONE,
    ))
}
