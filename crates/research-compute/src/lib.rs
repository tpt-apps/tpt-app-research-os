//! Cell execution for computational documents: a sandboxed Rhai engine, a shared
//! scope across cells, re-running of only the stale cells, and recording of runs
//! and results into `research-store`. The language decision is in
//! `docs/ARCHITECTURE.md`.

mod bindings;
mod engine;
mod error;
mod ode;
mod session;
mod units;

pub use engine::{Control, Limits, LANGUAGE};
pub use error::{ComputeError, Result};
pub use session::{Recording, RunSummary, Session};

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-compute";

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use research_core::{CellId, DocId, ProjectId, ProjectManifest, RunId};
    use research_doc::{Cell, CellBody, Document, Output};
    use research_store::{ProjectStore, Value};

    use super::*;

    fn code(id: &str, source: &str) -> Cell {
        Cell {
            id: CellId::new(id).unwrap(),
            body: CellBody::Code {
                language: LANGUAGE.into(),
                source: source.into(),
                outputs: Vec::new(),
            },
        }
    }

    fn document(cells: Vec<Cell>) -> Document {
        let mut doc = Document::new(DocId::new("doc").unwrap(), "Test");
        doc.cells = cells;
        doc
    }

    fn outputs_of(doc: &Document, id: &str) -> Vec<Output> {
        let cell = doc.cell(&CellId::new(id).unwrap()).unwrap();
        match &cell.body {
            CellBody::Code { outputs, .. } => outputs.clone(),
            other => panic!("expected code cell, got {other:?}"),
        }
    }

    fn text(s: &str) -> Output {
        Output::Text { text: s.into() }
    }

    fn error_name(doc: &Document, id: &str) -> String {
        match outputs_of(doc, id).as_slice() {
            [.., Output::Error { name, .. }] => name.clone(),
            other => panic!("expected an error output, got {other:?}"),
        }
    }

    fn session(seed: u64) -> Session {
        Session::new(seed, Limits::default()).unwrap()
    }

    fn run(session: &mut Session, doc: &mut Document) -> RunSummary {
        session.run_document(doc, &Control::new(), None).unwrap()
    }

    #[test]
    fn cells_run_in_order_with_a_shared_scope() {
        let mut doc = document(vec![
            code("a", "let x = 2;"),
            code("b", "print(\"hi\"); x * 3"),
        ]);
        let summary = run(&mut session(1), &mut doc);
        assert_eq!(summary.ran.len(), 2);
        assert_eq!(outputs_of(&doc, "b"), vec![text("hi"), text("6")]);
    }

    #[test]
    fn unchanged_cells_are_reused_on_the_next_run() {
        let mut doc = document(vec![code("a", "let x = 2;"), code("b", "x + 1")]);
        let mut s = session(1);
        run(&mut s, &mut doc);
        let second = run(&mut s, &mut doc);
        assert!(second.ran.is_empty());
        assert_eq!(second.reused.len(), 2);
    }

    #[test]
    fn editing_a_cell_reruns_it_and_everything_after() {
        let mut doc = document(vec![
            code("a", "let x = 2;"),
            code("b", "x + 1"),
            code("c", "x + 2"),
        ]);
        let mut s = session(1);
        run(&mut s, &mut doc);

        doc.cells[2].body = CellBody::Code {
            language: LANGUAGE.into(),
            source: "x + 20".into(),
            outputs: Vec::new(),
        };
        let summary = run(&mut s, &mut doc);
        assert_eq!(summary.ran, vec![CellId::new("c").unwrap()]);
        assert_eq!(
            summary.reused,
            vec![CellId::new("a").unwrap(), CellId::new("b").unwrap()]
        );

        doc.cells[0].body = CellBody::Code {
            language: LANGUAGE.into(),
            source: "let x = 10;".into(),
            outputs: Vec::new(),
        };
        let summary = run(&mut s, &mut doc);
        assert_eq!(
            summary.ran.len(),
            3,
            "a change upstream reruns all later cells"
        );
        assert_eq!(outputs_of(&doc, "c"), vec![text("30")]);
    }

    #[test]
    fn an_error_is_stored_and_later_cells_still_run() {
        let mut doc = document(vec![code("bad", "let = ;"), code("good", "7")]);
        run(&mut session(1), &mut doc);
        assert_eq!(error_name(&doc, "bad"), "RuntimeError");
        assert_eq!(outputs_of(&doc, "good"), vec![text("7")]);
    }

    #[test]
    fn operation_limit_stops_a_runaway_loop() {
        let limits = Limits {
            max_operations: 10_000,
            ..Limits::default()
        };
        let mut s = Session::new(1, limits).unwrap();
        let mut doc = document(vec![code("loop", "let i = 0; while true { i += 1; }")]);
        run(&mut s, &mut doc);
        assert_eq!(error_name(&doc, "loop"), "OperationLimit");
    }

    #[test]
    fn timeout_stops_a_slow_cell() {
        let limits = Limits {
            max_operations: u64::MAX / 2,
            timeout: Duration::from_millis(50),
            ..Limits::default()
        };
        let mut s = Session::new(1, limits).unwrap();
        let mut doc = document(vec![code("slow", "let i = 0; while true { i += 1; }")]);
        run(&mut s, &mut doc);
        assert_eq!(error_name(&doc, "slow"), "Timeout");
    }

    #[test]
    fn memory_limit_stops_string_growth() {
        let limits = Limits {
            max_string_bytes: 1024,
            ..Limits::default()
        };
        let mut s = Session::new(1, limits).unwrap();
        let mut doc = document(vec![code(
            "grow",
            "let s = \"x\"; for i in 0..40 { s += s; } s.len()",
        )]);
        run(&mut s, &mut doc);
        assert_eq!(error_name(&doc, "grow"), "MemoryLimit");
    }

    #[test]
    fn cancellation_before_the_run_runs_nothing() {
        let mut doc = document(vec![code("a", "1")]);
        let control = Control::new();
        control.cancel();
        let summary = session(1).run_document(&mut doc, &control, None).unwrap();
        assert!(summary.cancelled);
        assert!(summary.ran.is_empty());
        assert!(outputs_of(&doc, "a").is_empty());
    }

    #[test]
    fn host_file_access_is_not_available() {
        let mut doc = document(vec![code("io", "open_file(\"/etc/passwd\")")]);
        run(&mut session(1), &mut doc);
        let outputs = outputs_of(&doc, "io");
        match outputs.as_slice() {
            [Output::Error { name, message }] => {
                assert_eq!(name, "RuntimeError");
                assert!(message.contains("open_file"), "{message}");
            }
            other => panic!("unexpected outputs {other:?}"),
        }
    }

    #[test]
    fn uniform_is_reproducible_for_the_same_seed() {
        let mut first = document(vec![code("u", "uniform(3)")]);
        let mut second = document(vec![code("u", "uniform(3)")]);
        let mut other_seed = document(vec![code("u", "uniform(3)")]);
        run(&mut session(42), &mut first);
        run(&mut session(42), &mut second);
        run(&mut session(43), &mut other_seed);
        assert_eq!(outputs_of(&first, "u"), outputs_of(&second, "u"));
        assert_ne!(outputs_of(&first, "u"), outputs_of(&other_seed, "u"));
    }

    #[test]
    fn cells_in_other_languages_are_not_run() {
        let mut doc = document(vec![Cell {
            id: CellId::new("py").unwrap(),
            body: CellBody::Code {
                language: "python".into(),
                source: "print(1)".into(),
                outputs: Vec::new(),
            },
        }]);
        let summary = run(&mut session(1), &mut doc);
        assert!(summary.ran.is_empty());
        assert_eq!(error_name(&doc, "py"), "UnsupportedLanguage");
    }

    #[test]
    fn runs_and_results_are_recorded_in_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = ProjectManifest::new(ProjectId::new("p").unwrap(), "P", 0).unwrap();
        let store = ProjectStore::create(&dir.path().join("p"), &manifest).unwrap();

        let mut doc = document(vec![code("a", "let x = 2;"), code("b", "x * 1.5")]);
        let recording = Recording {
            store: &store,
            run_prefix: "doc-1",
        };
        session(9)
            .run_document(&mut doc, &Control::new(), Some(recording))
            .unwrap();

        let runs = store.list_runs().unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs
            .iter()
            .all(|r| r.inputs.get("seed") == Some(&"9".to_string())));

        let with_value: Vec<_> = runs
            .iter()
            .flat_map(|r| store.results_for_run(&r.id).unwrap())
            .filter(|r| r.name == "value")
            .collect();
        assert_eq!(with_value.len(), 1);
        assert_eq!(with_value[0].value, Value::Scalar { value: 3.0 });
        assert!(store
            .load_run(&RunId::new("missing").unwrap())
            .unwrap()
            .is_none());
    }

    #[test]
    fn limits_that_cannot_work_are_refused() {
        let zero = Limits {
            timeout: Duration::ZERO,
            ..Limits::default()
        };
        assert!(Session::new(1, zero).is_err());
    }

    #[test]
    fn normal_draws_have_the_requested_mean_and_are_reproducible() {
        let source = "let s = 0.0; for i in 0..4000 { s += normal(i, 5.0, 1.0); } s / 4000.0";
        let mut first = document(vec![code("n", source)]);
        let mut second = document(vec![code("n", source)]);
        run(&mut session(7), &mut first);
        run(&mut session(7), &mut second);
        assert_eq!(outputs_of(&first, "n"), outputs_of(&second, "n"));
        let mean: f64 = match outputs_of(&first, "n").as_slice() {
            [Output::Text { text }] => text.parse().unwrap(),
            other => panic!("unexpected outputs {other:?}"),
        };
        assert!((mean - 5.0).abs() < 0.1, "mean was {mean}");
    }

    #[test]
    fn solve_returns_the_linear_system_solution() {
        // 2x + y = 3 and x + 3y = 5 give x = 0.8, y = 1.4.
        let mut doc = document(vec![code(
            "s",
            "let x = solve([[2.0, 1.0], [1.0, 3.0]], [3.0, 5.0]); \
             (x[0] * 10.0).round() + (x[1] * 10.0).round()",
        )]);
        run(&mut session(1), &mut doc);
        assert_eq!(outputs_of(&doc, "s"), vec![text("22.0")]);
    }

    #[test]
    fn ode_solve_integrates_exponential_decay_with_parameters() {
        // y' = -k y with k = 2 from y(0) = 1: y(1) = e^-2 = 0.1353...
        let mut doc = document(vec![code(
            "ode",
            "let r = ode_solve(\"[-k * y[0]]\", #{k: 2.0}, [1.0], 0.0, [0.5, 1.0], \"tsit45\"); \
             (r[1][0] * 1000.0).round()",
        )]);
        run(&mut session(1), &mut doc);
        assert_eq!(outputs_of(&doc, "ode"), vec![text("135.0")]);
    }

    #[test]
    fn ode_solve_handles_stiff_problems_with_the_implicit_method() {
        let mut doc = document(vec![code(
            "stiff",
            "let r = ode_solve(\"[-1000.0 * (y[0] - cos(t))]\", #{}, [0.0], 0.0, [1.0], \"bdf\"); \
             (r[0][0] * 1000.0).round()",
        )]);
        run(&mut session(1), &mut doc);
        // After a fast transient the solution sits on the slow manifold cos(t) + sin(t)/1000,
        // which is 0.5411 at t = 1.
        assert_eq!(outputs_of(&doc, "stiff"), vec![text("541.0")]);
    }

    #[test]
    fn show_table_produces_a_table_output_after_printed_text() {
        let mut doc = document(vec![code(
            "t",
            "print(\"header\"); show_table([\"t\", \"x\"], [[0.0, 1.0], [1.0, 0.5]])",
        )]);
        run(&mut session(1), &mut doc);
        assert_eq!(
            outputs_of(&doc, "t"),
            vec![
                text("header"),
                Output::Table {
                    columns: vec!["t".into(), "x".into()],
                    rows: vec![
                        vec!["0.0".into(), "1.0".into()],
                        vec!["1.0".into(), "0.5".into()],
                    ],
                },
            ]
        );
    }

    #[test]
    fn show_table_refuses_ragged_rows_and_empty_columns() {
        let mut doc = document(vec![
            code("ragged", "show_table([\"a\", \"b\"], [[1.0]])"),
            code("empty", "show_table([], [])"),
        ]);
        run(&mut session(1), &mut doc);
        assert_eq!(error_name(&doc, "ragged"), "RuntimeError");
        assert_eq!(error_name(&doc, "empty"), "RuntimeError");
    }

    #[test]
    fn ode_solve_reports_bad_inputs_as_cell_errors() {
        let mut doc = document(vec![
            code(
                "syntax",
                "ode_solve(\"[-y[0] +\", #{}, [1.0], 0.0, [1.0], \"bdf\")",
            ),
            code(
                "shape",
                "ode_solve(\"[1.0, 2.0]\", #{}, [1.0], 0.0, [1.0], \"bdf\")",
            ),
            code(
                "method",
                "ode_solve(\"[0.0]\", #{}, [1.0], 0.0, [1.0], \"rk9\")",
            ),
            code(
                "param",
                "ode_solve(\"[k]\", #{k: \"two\"}, [1.0], 0.0, [1.0], \"bdf\")",
            ),
        ]);
        run(&mut session(1), &mut doc);
        for id in ["syntax", "shape", "method", "param"] {
            assert_eq!(error_name(&doc, id), "RuntimeError", "cell {id}");
        }
    }

    #[test]
    fn solve_refuses_bad_shapes_and_singular_systems() {
        let mut doc = document(vec![
            code("ragged", "solve([[1.0, 2.0], [3.0]], [1.0, 2.0])"),
            code("short", "solve([[1.0, 0.0], [0.0, 1.0]], [1.0])"),
            code("singular", "solve([[1.0, 2.0], [2.0, 4.0]], [1.0, 2.0])"),
        ]);
        run(&mut session(1), &mut doc);
        for id in ["ragged", "short", "singular"] {
            assert_eq!(error_name(&doc, id), "RuntimeError", "cell {id}");
        }
    }
}
