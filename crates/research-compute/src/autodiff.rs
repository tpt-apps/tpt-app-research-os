//! `diff`: the derivative of an expression at a point, by forward-mode automatic
//! differentiation (dual numbers). Exact to rounding, unlike finite differences.
//!
//! The expression is a string over `x`, the variable to differentiate, and the
//! entries of an optional parameter map, which are constants. It may use `+ - * /`,
//! unary minus, and `sin cos tan tanh atan exp ln sqrt abs powi powf`.

use rhai::{Dynamic, Engine, EvalAltResult, Map, Scope};
use tpt_math_autodiff_fwd::Dual;

use crate::engine::Limits;
use crate::ode::{parameters, runtime};

/// A value that carries its derivative with respect to `x`.
#[derive(Clone, Copy)]
struct Ad(Dual<f64>);

/// The derivative of `expr` with respect to `x`, evaluated at `x`.
pub(crate) fn diff(
    expr: String,
    x: f64,
    params: Map,
    limits: &Limits,
) -> Result<f64, Box<EvalAltResult>> {
    let constants = parameters(params)?;

    let mut engine = Engine::new();
    engine.set_max_operations(limits.max_operations);
    engine.set_max_string_size(limits.max_string_bytes);
    engine.set_max_array_size(limits.max_collection_len);
    engine.set_max_call_levels(limits.max_call_depth);
    register(&mut engine);
    let ast = engine
        .compile_expression(&expr)
        .map_err(|e| runtime(&format!("expression does not compile: {e}")))?;

    let mut scope = Scope::new();
    for (name, value) in constants {
        scope.push_constant(name, value);
    }
    scope.push("x", Ad(Dual::variable(x, 0)));
    let value: Dynamic = engine.eval_ast_with_scope(&mut scope, &ast)?;
    let result = value
        .clone()
        .try_cast::<Ad>()
        .or_else(|| value.try_cast::<f64>().map(|c| Ad(Dual::constant(c))))
        .ok_or_else(|| runtime("the expression must evaluate to a number"))?;
    Ok(result.0.du(0))
}

fn register(engine: &mut Engine) {
    engine.register_type_with_name::<Ad>("Dual");

    engine.register_fn("+", |a: Ad, b: Ad| Ad(a.0 + b.0));
    engine.register_fn("-", |a: Ad, b: Ad| Ad(a.0 - b.0));
    engine.register_fn("*", |a: Ad, b: Ad| Ad(a.0 * b.0));
    engine.register_fn("/", |a: Ad, b: Ad| Ad(a.0 / b.0));
    engine.register_fn("-", |a: Ad| Ad(a.0.negate()));

    // Mixing with plain numbers. Integer literals such as `2` are accepted too.
    engine.register_fn("+", |a: Ad, b: f64| Ad(a.0 + b));
    engine.register_fn("-", |a: Ad, b: f64| Ad(a.0 - b));
    engine.register_fn("*", |a: Ad, b: f64| Ad(a.0 * b));
    engine.register_fn("/", |a: Ad, b: f64| Ad(a.0 / b));
    engine.register_fn("+", |a: f64, b: Ad| Ad(Dual::constant(a) + b.0));
    engine.register_fn("-", |a: f64, b: Ad| Ad(Dual::constant(a) - b.0));
    engine.register_fn("*", |a: f64, b: Ad| Ad(Dual::constant(a) * b.0));
    engine.register_fn("/", |a: f64, b: Ad| Ad(Dual::constant(a) / b.0));
    engine.register_fn("+", |a: Ad, b: i64| Ad(a.0 + b as f64));
    engine.register_fn("-", |a: Ad, b: i64| Ad(a.0 - b as f64));
    engine.register_fn("*", |a: Ad, b: i64| Ad(a.0 * b as f64));
    engine.register_fn("/", |a: Ad, b: i64| Ad(a.0 / b as f64));
    engine.register_fn("+", |a: i64, b: Ad| Ad(Dual::constant(a as f64) + b.0));
    engine.register_fn("-", |a: i64, b: Ad| Ad(Dual::constant(a as f64) - b.0));
    engine.register_fn("*", |a: i64, b: Ad| Ad(Dual::constant(a as f64) * b.0));
    engine.register_fn("/", |a: i64, b: Ad| Ad(Dual::constant(a as f64) / b.0));

    engine.register_fn("sin", |a: Ad| Ad(a.0.sin()));
    engine.register_fn("cos", |a: Ad| Ad(a.0.cos()));
    engine.register_fn("tan", |a: Ad| Ad(a.0.tan()));
    engine.register_fn("tanh", |a: Ad| Ad(a.0.tanh()));
    engine.register_fn("atan", |a: Ad| Ad(a.0.atan()));
    engine.register_fn("exp", |a: Ad| Ad(a.0.exp()));
    engine.register_fn("ln", |a: Ad| Ad(a.0.ln()));
    engine.register_fn("sqrt", |a: Ad| Ad(a.0.sqrt()));
    engine.register_fn("abs", |a: Ad| Ad(a.0.abs()));
    engine.register_fn("powi", |a: Ad, n: i64| Ad(a.0.powi(n as i32)));
    engine.register_fn("powf", |a: Ad, p: f64| Ad(a.0.powf(p)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(expr: &str, x: f64) -> Result<f64, Box<EvalAltResult>> {
        diff(expr.into(), x, Map::new(), &Limits::default())
    }

    #[test]
    fn derivatives_of_polynomials_and_products_are_exact() {
        assert_eq!(d("x * x", 3.0).unwrap(), 6.0);
        assert_eq!(d("x * x * x - 2.0 * x", 2.0).unwrap(), 10.0);
        assert!((d("x * sin(x)", 0.0).unwrap()).abs() < 1e-15);
    }

    #[test]
    fn quotients_and_elementary_functions_follow_the_chain_rule() {
        // d/dx x/(1+x) = 1/(1+x)^2.
        assert!((d("x / (1.0 + x)", 1.0).unwrap() - 0.25).abs() < 1e-15);
        assert!((d("sin(x)", 0.0).unwrap() - 1.0).abs() < 1e-15);
        assert!((d("exp(x)", 1.0).unwrap() - std::f64::consts::E).abs() < 1e-14);
        assert!((d("ln(x)", 2.0).unwrap() - 0.5).abs() < 1e-15);
        assert!((d("sqrt(x)", 4.0).unwrap() - 0.25).abs() < 1e-15);
    }

    #[test]
    fn integer_literals_and_constants_work() {
        assert_eq!(d("x * 2", 1.0).unwrap(), 2.0);
        assert_eq!(d("powi(x, 3)", 2.0).unwrap(), 12.0);
        let mut params = Map::new();
        params.insert("k".into(), Dynamic::from(2.0_f64));
        let got = diff("exp(k * x)".into(), 0.0, params, &Limits::default()).unwrap();
        assert!((got - 2.0).abs() < 1e-15);
    }

    #[test]
    fn a_constant_expression_has_zero_derivative() {
        assert_eq!(d("3.0", 1.0).unwrap(), 0.0);
    }

    #[test]
    fn bad_expressions_are_cell_errors() {
        assert!(d("x +", 1.0).is_err(), "does not compile");
        assert!(d("y * x", 1.0).is_err(), "unknown name");
        assert!(d("\"text\"", 1.0).is_err(), "not a number");
    }
}
