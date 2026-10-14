//! Dimension-checked quantities for cells, built on `tpt-math-units-dyn`.
//!
//! `qty(2.0, "km")` makes a quantity. `+` and `-` need matching dimensions, so adding
//! metres to seconds is a cell error, not a wrong number. `*` and `/` combine
//! dimensions. `qty_in(q, "m")` converts a quantity to a unit and returns the number.

use rhai::{Dynamic, Engine, EvalAltResult, Position};
use tpt_math_units_dyn::{DynQuantity, UnitError};

pub(crate) fn register(engine: &mut Engine) {
    engine.register_type_with_name::<DynQuantity>("Quantity");

    engine.register_fn(
        "qty",
        |value: f64, unit: String| -> Result<DynQuantity, Box<EvalAltResult>> {
            DynQuantity::parse(value, &unit).map_err(unit_error)
        },
    );
    engine.register_fn(
        "qty_in",
        |quantity: DynQuantity, unit: String| -> Result<f64, Box<EvalAltResult>> {
            quantity.convert_to(&unit).map_err(unit_error)
        },
    );
    engine.register_fn(
        "+",
        |a: DynQuantity, b: DynQuantity| -> Result<DynQuantity, Box<EvalAltResult>> {
            a.try_add(&b).map_err(unit_error)
        },
    );
    engine.register_fn(
        "-",
        |a: DynQuantity, b: DynQuantity| -> Result<DynQuantity, Box<EvalAltResult>> {
            a.try_sub(&b).map_err(unit_error)
        },
    );
    engine.register_fn(
        "*",
        |a: DynQuantity, b: DynQuantity| -> Result<DynQuantity, Box<EvalAltResult>> {
            a.try_mul(&b).map_err(unit_error)
        },
    );
    engine.register_fn(
        "/",
        |a: DynQuantity, b: DynQuantity| -> Result<DynQuantity, Box<EvalAltResult>> {
            a.try_div(&b).map_err(unit_error)
        },
    );
    engine.register_fn("to_string", |q: DynQuantity| q.to_string());
}

fn unit_error(error: UnitError) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(error.to_string()),
        Position::NONE,
    ))
}

#[cfg(test)]
mod tests {
    use crate::engine::Limits;
    use crate::session::Session;
    use crate::{Control, RunSummary};
    use research_core::{CellId, DocId};
    use research_doc::{Cell, CellBody, Document, Output};

    fn run_cells(sources: &[&str]) -> Document {
        let mut doc = Document::new(DocId::new("u").unwrap(), "Units");
        for (i, source) in sources.iter().enumerate() {
            doc.cells.push(Cell {
                id: CellId::new(format!("c{i}")).unwrap(),
                body: CellBody::Code {
                    language: "rhai".into(),
                    source: (*source).into(),
                    outputs: Vec::new(),
                },
            });
        }
        let summary: RunSummary = Session::new(1, Limits::default())
            .unwrap()
            .run_document(&mut doc, &Control::new(), None)
            .unwrap();
        assert_eq!(summary.ran.len(), sources.len());
        doc
    }

    fn outputs(doc: &Document, index: usize) -> Vec<Output> {
        match &doc.cells[index].body {
            CellBody::Code { outputs, .. } => outputs.clone(),
            other => panic!("expected code cell, got {other:?}"),
        }
    }

    #[test]
    fn same_dimension_sums_convert_to_the_requested_unit() {
        let doc = run_cells(&["qty_in(qty(3.0, \"km\") + qty(500.0, \"m\"), \"m\")"]);
        assert_eq!(
            outputs(&doc, 0),
            vec![Output::Text {
                text: "3500.0".into()
            }]
        );
    }

    #[test]
    fn adding_different_dimensions_is_a_cell_error() {
        let doc = run_cells(&["qty(1.0, \"m\") + qty(1.0, \"s\")"]);
        match outputs(&doc, 0).as_slice() {
            [Output::Error { name, message }] => {
                assert_eq!(name, "RuntimeError");
                assert!(message.to_lowercase().contains("dimension"), "{message}");
            }
            other => panic!("unexpected outputs {other:?}"),
        }
    }

    #[test]
    fn products_combine_dimensions_and_unknown_units_are_errors() {
        let doc = run_cells(&[
            "qty_in(qty(2.0, \"m\") * qty(3.0, \"m\"), \"m2\")",
            "qty(1.0, \"parsecs-per-fortnight-ish\")",
        ]);
        assert_eq!(outputs(&doc, 0), vec![Output::Text { text: "6.0".into() }]);
        assert!(matches!(
            outputs(&doc, 1).as_slice(),
            [Output::Error { name, .. }] if name == "RuntimeError"
        ));
    }
}
