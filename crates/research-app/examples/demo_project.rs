//! Build the example research project: two documents that use the compute layer.
//!
//! `cargo run -p research-app --example demo_project -- <dir>` creates the project in
//! `<dir>` (default `demo-project`), runs both documents, and prints each cell's
//! outputs. The project opens in the editor with `cargo run -p research-app -- <dir>`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use research_app::Workspace;
use research_compute::Control;
use research_core::CellId;
use research_doc::{Cell, CellBody, Document, Output};

fn code(id: &str, source: &str) -> Cell {
    Cell {
        id: CellId::new(id).expect("demo ids are valid"),
        body: CellBody::Code {
            language: research_compute::LANGUAGE.into(),
            source: source.into(),
            outputs: Vec::new(),
        },
    }
}

fn text(id: &str, source: &str) -> Cell {
    Cell {
        id: CellId::new(id).expect("demo ids are valid"),
        body: CellBody::Markdown {
            source: source.into(),
        },
    }
}

fn oscillator() -> Vec<Cell> {
    vec![
        text(
            "intro",
            "# Damped decay\n\nSolve y' = -k y with k = 2 and y(0) = 1, then compare with e^-2.",
        ),
        code(
            "solve",
            "let r = ode_solve(\"[-k * y[0]]\", #{k: 2.0}, [1.0], 0.0, [0.5, 1.0], \"tsit45\");\n\
             show_table([\"t\", \"y\"], [[0.5, r[0][0]], [1.0, r[1][0]]]);\n\
             r[1][0]",
        ),
        code(
            "compare",
            "let exact = 0.1353352832;\n\
             let got = ode_solve(\"[-k * y[0]]\", #{k: 2.0}, [1.0], 0.0, [1.0], \"tsit45\")[0][0];\n\
             \"error: \" + to_string(got - exact)",
        ),
        code(
            "length",
            "let distance = qty(3.0, \"km\") + qty(500.0, \"m\");\n\
             qty_in(distance, \"m\")",
        ),
    ]
}

fn sampling() -> Vec<Cell> {
    vec![
        text(
            "intro",
            "# Sampling and a linear solve\n\nDraw normal samples from a fixed seed, then solve a 2 by 2 system.",
        ),
        code(
            "mean",
            "let total = 0.0;\n\
             for i in 0..1000 { total += normal(i, 5.0, 1.0); }\n\
             total / 1000.0",
        ),
        code(
            "system",
            "let x = solve([[2.0, 1.0], [1.0, 3.0]], [3.0, 5.0]);\n\
             show_table([\"unknown\", \"value\"], [[\"x\", x[0]], [\"y\", x[1]]])",
        ),
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("demo-project"));
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut ws = Workspace::create(&dir, "demo-project", "Demo project", now)?;

    for (id, title, cells) in [
        ("damped-decay", "Damped decay", oscillator()),
        ("sampling", "Sampling and a solve", sampling()),
    ] {
        let mut doc: Document = ws.new_document(id, title)?;
        doc.cells = cells;
        let summary = ws.run_document(&mut doc, &Control::new())?;
        println!("{title}: ran {} cell(s)", summary.ran.len());
        for cell in &doc.cells {
            if let CellBody::Code { outputs, .. } = &cell.body {
                for output in outputs {
                    match output {
                        Output::Text { text } => println!("  {}: {text}", cell.id),
                        Output::Error { name, message } => {
                            println!("  {}: {name}: {message}", cell.id)
                        }
                        Output::Table { columns, rows } => {
                            println!("  {}: table {columns:?}", cell.id);
                            for row in rows {
                                println!("    {row:?}");
                            }
                        }
                    }
                }
            }
        }
    }
    println!("project written to {}", dir.display());
    Ok(())
}
