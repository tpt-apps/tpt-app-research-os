//! Jupyter notebook (nbformat 4) import and export. Mapping is described in
//! `docs/DOCUMENT_FORMAT.md`.

use std::collections::BTreeMap;
use std::collections::HashSet;

use research_core::{CellId, DocId};
use serde_json::{json, Value};

use crate::document::{Cell, CellBody, Document, Output, FORMAT_VERSION};
use crate::error::{DocError, Result};

/// Notebook metadata key that holds document-level fields.
const RESEARCH_KEY: &str = "research_os";

/// Result of importing a notebook.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    pub document: Document,
    /// Things that were dropped, or that will not run, in reading order.
    pub warnings: Vec<String>,
}

/// Import an nbformat 4 notebook.
///
/// The languages the compute layer runs. Cells in any other language import with a
/// warning and do not run. Rhai is the only one (see `docs/ARCHITECTURE.md`).
pub const HOSTED_LANGUAGES: &[&str] = &["rhai"];

/// `hosted_languages` lists the languages the compute layer can run. A code cell
/// in any other language is still imported, with a warning.
pub fn import_ipynb(json: &str, hosted_languages: &[&str]) -> Result<ImportReport> {
    let nb: Value = serde_json::from_str(json)?;
    match nb.get("nbformat").and_then(Value::as_u64) {
        Some(4) => {}
        other => {
            return Err(DocError::Notebook(format!(
                "only nbformat 4 is supported (found {})",
                other.map_or("none".to_string(), |v| v.to_string())
            )))
        }
    }

    let metadata = nb.get("metadata").cloned().unwrap_or(Value::Null);
    let research = metadata.get(RESEARCH_KEY).cloned().unwrap_or(Value::Null);
    let kernel_language = metadata
        .pointer("/kernelspec/language")
        .or_else(|| metadata.pointer("/language_info/name"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let doc_id = research
        .get("id")
        .and_then(Value::as_str)
        .map(DocId::new)
        .transpose()?
        .unwrap_or(DocId::new("notebook-import")?);
    let mut document = Document::new(
        doc_id,
        research.get("title").and_then(Value::as_str).unwrap_or(""),
    );
    if let Some(map) = research.get("metadata").and_then(Value::as_object) {
        document.metadata = map
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
            .collect::<BTreeMap<_, _>>();
    }

    let cells = nb
        .get("cells")
        .and_then(Value::as_array)
        .ok_or_else(|| DocError::Notebook("notebook has no cells array".into()))?;

    let mut warnings = Vec::new();
    let mut used = HashSet::new();
    for (index, cell) in cells.iter().enumerate() {
        let id = resolve_cell_id(cell.get("id"), index, &mut used);
        let cell_type = cell.get("cell_type").and_then(Value::as_str).unwrap_or("");
        match cell_type {
            "markdown" => document.cells.push(Cell {
                id,
                body: CellBody::Markdown {
                    source: multiline(cell.get("source")),
                },
            }),
            "code" => {
                let language = cell
                    .pointer(&format!("/metadata/{RESEARCH_KEY}/language"))
                    .and_then(Value::as_str)
                    .unwrap_or(&kernel_language)
                    .to_string();
                if !hosted_languages.contains(&language.as_str()) {
                    warnings.push(format!(
                        "cell {id} is {language:?}, which the compute layer cannot run"
                    ));
                }
                let outputs = convert_outputs(cell.get("outputs"), &id, &mut warnings);
                document.cells.push(Cell {
                    id,
                    body: CellBody::Code {
                        language,
                        source: multiline(cell.get("source")),
                        outputs,
                    },
                });
            }
            other => warnings.push(format!(
                "cell {id} of type {other:?} was skipped (only markdown and code are imported)"
            )),
        }
    }

    document.validate()?;
    Ok(ImportReport { document, warnings })
}

/// Export a document as an nbformat 4.5 notebook.
///
/// Fields that have no notebook equivalent (document title, metadata, cell
/// languages) are written under `metadata.research_os` so import can restore them.
pub fn export_ipynb(doc: &Document) -> Result<String> {
    doc.validate()?;
    let kernel = doc
        .cells
        .iter()
        .find_map(|c| match &c.body {
            CellBody::Code { language, .. } => Some(language.as_str()),
            _ => None,
        })
        .unwrap_or("text");

    let mut cells = Vec::with_capacity(doc.cells.len());
    for cell in &doc.cells {
        if cell.id.as_str().len() > 64 {
            return Err(DocError::Invalid(format!(
                "cell id {} is longer than 64 characters, which nbformat does not allow",
                cell.id
            )));
        }
        cells.push(match &cell.body {
            CellBody::Markdown { source } => json!({
                "cell_type": "markdown",
                "id": cell.id.as_str(),
                "metadata": {},
                "source": source_lines(source),
            }),
            CellBody::Code {
                language,
                source,
                outputs,
            } => json!({
                "cell_type": "code",
                "id": cell.id.as_str(),
                "metadata": { RESEARCH_KEY: { "language": language } },
                "execution_count": null,
                "source": source_lines(source),
                "outputs": outputs.iter().map(export_output).collect::<Vec<_>>(),
            }),
        });
    }

    let notebook = json!({
        "cells": cells,
        "metadata": {
            "kernelspec": { "name": kernel, "display_name": kernel, "language": kernel },
            "language_info": { "name": kernel },
            RESEARCH_KEY: {
                "id": doc.id.as_str(),
                "title": doc.title,
                "metadata": doc.metadata,
                "format_version": FORMAT_VERSION,
            },
        },
        "nbformat": 4,
        "nbformat_minor": 5,
    });
    let mut out = serde_json::to_string_pretty(&notebook)?;
    out.push('\n');
    Ok(out)
}

fn export_output(output: &Output) -> Value {
    match output {
        Output::Text { text } => json!({
            "output_type": "stream",
            "name": "stdout",
            "text": source_lines(text),
        }),
        Output::Error { name, message } => json!({
            "output_type": "error",
            "ename": name,
            "evalue": message,
            "traceback": [],
        }),
        Output::Table { columns, rows } => {
            let mut text = columns.join("\t");
            for row in rows {
                text.push('\n');
                text.push_str(&row.join("\t"));
            }
            json!({
                "output_type": "display_data",
                "data": { "text/plain": source_lines(&text) },
                "metadata": {},
            })
        }
        Output::Svg { svg } => json!({
            "output_type": "display_data",
            "data": {
                "image/svg+xml": source_lines(svg),
                "text/plain": ["[plot]"],
            },
            "metadata": {},
        }),
    }
}

fn convert_outputs(
    outputs: Option<&Value>,
    cell: &CellId,
    warnings: &mut Vec<String>,
) -> Vec<Output> {
    let Some(items) = outputs.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut converted = Vec::new();
    for item in items {
        match item.get("output_type").and_then(Value::as_str) {
            Some("stream") => converted.push(Output::Text {
                text: multiline(item.get("text")),
            }),
            Some("execute_result") | Some("display_data") => {
                let data = item.get("data").and_then(Value::as_object);
                if let Some(svg) = data.and_then(|d| d.get("image/svg+xml")) {
                    // A plot keeps its SVG. Its text/plain is only a label.
                    converted.push(Output::Svg {
                        svg: multiline(Some(svg)),
                    });
                    continue;
                }
                let text = data.and_then(|d| d.get("text/plain"));
                match text {
                    Some(text) => converted.push(Output::Text {
                        text: multiline(Some(text)),
                    }),
                    None => warnings.push(format!("cell {cell}: a rich output was dropped")),
                }
                if let Some(data) = data {
                    let others: Vec<&str> = data
                        .keys()
                        .filter(|k| k.as_str() != "text/plain")
                        .map(String::as_str)
                        .collect();
                    if text.is_some() && !others.is_empty() {
                        warnings.push(format!(
                            "cell {cell}: dropped rich formats {}",
                            others.join(", ")
                        ));
                    }
                }
            }
            Some("error") => converted.push(Output::Error {
                name: item
                    .get("ename")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                message: item
                    .get("evalue")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            }),
            other => warnings.push(format!(
                "cell {cell}: output of type {} was dropped",
                other.unwrap_or("unknown")
            )),
        }
    }
    converted
}

/// Use the notebook's own cell id if it is valid and unused, otherwise make one
/// from the cell's position.
fn resolve_cell_id(raw: Option<&Value>, index: usize, used: &mut HashSet<String>) -> CellId {
    let candidate = raw
        .and_then(Value::as_str)
        .and_then(|s| CellId::new(s).ok())
        .filter(|id| !used.contains(id.as_str()));
    let id = candidate.unwrap_or_else(|| {
        let mut name = format!("imported-{index}");
        let mut n = 1;
        while used.contains(&name) {
            name = format!("imported-{index}-{n}");
            n += 1;
        }
        CellId::new(name).expect("generated ids are valid")
    });
    used.insert(id.as_str().to_string());
    id
}

/// nbformat stores multi-line text as a string or as an array of lines.
fn multiline(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// Split text into nbformat lines, each keeping its newline.
fn source_lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOSTED: &[&str] = HOSTED_LANGUAGES;

    fn notebook(cells: &str, kernel: &str) -> String {
        format!(
            r#"{{
  "cells": {cells},
  "metadata": {{ "kernelspec": {{ "name": "{kernel}", "language": "{kernel}" }} }},
  "nbformat": 4,
  "nbformat_minor": 5
}}"#
        )
    }

    #[test]
    fn imports_markdown_code_and_outputs() {
        let nb = notebook(
            r#"[
              {"cell_type":"markdown","id":"intro","metadata":{},"source":["Title\n","body"]},
              {"cell_type":"code","id":"run","metadata":{},"execution_count":3,
               "source":"let x = 1;",
               "outputs":[
                 {"output_type":"stream","name":"stdout","text":["hello\n"]},
                 {"output_type":"execute_result","data":{"text/plain":"1"},"metadata":{},"execution_count":3},
                 {"output_type":"error","ename":"Oops","evalue":"bad","traceback":["x"]}
               ]}
            ]"#,
            "rhai",
        );
        let report = import_ipynb(&nb, HOSTED).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let cells = &report.document.cells;
        assert_eq!(cells[0].id.as_str(), "intro");
        assert_eq!(
            cells[0].body,
            CellBody::Markdown {
                source: "Title\nbody".into()
            }
        );
        match &cells[1].body {
            CellBody::Code {
                language,
                source,
                outputs,
            } => {
                assert_eq!(language, "rhai");
                assert_eq!(source, "let x = 1;");
                assert_eq!(
                    outputs,
                    &vec![
                        Output::Text {
                            text: "hello\n".into()
                        },
                        Output::Text { text: "1".into() },
                        Output::Error {
                            name: "Oops".into(),
                            message: "bad".into()
                        },
                    ]
                );
            }
            other => panic!("expected code cell, got {other:?}"),
        }
    }

    #[test]
    fn cells_in_unhosted_languages_are_imported_with_a_warning() {
        let nb = notebook(
            r#"[{"cell_type":"code","id":"py","metadata":{},"source":"print(1)","outputs":[]}]"#,
            "python3",
        );
        let report = import_ipynb(&nb, HOSTED).unwrap();
        assert_eq!(report.document.cells.len(), 1);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("python3"));
    }

    #[test]
    fn rich_outputs_and_raw_cells_are_reported() {
        let nb = notebook(
            r#"[
              {"cell_type":"raw","id":"r","metadata":{},"source":"raw"},
              {"cell_type":"code","id":"c","metadata":{},"source":"plot()","outputs":[
                {"output_type":"display_data","data":{"image/png":"AAAA","text/plain":"<Figure>"},"metadata":{}}
              ]}
            ]"#,
            "rhai",
        );
        let report = import_ipynb(&nb, HOSTED).unwrap();
        assert_eq!(report.document.cells.len(), 1);
        assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
        assert!(report.warnings.iter().any(|w| w.contains("raw")));
        assert!(report.warnings.iter().any(|w| w.contains("image/png")));
    }

    #[test]
    fn missing_or_duplicate_cell_ids_get_generated_ones() {
        let nb = notebook(
            r#"[
              {"cell_type":"markdown","id":"same","metadata":{},"source":"a"},
              {"cell_type":"markdown","id":"same","metadata":{},"source":"b"},
              {"cell_type":"markdown","metadata":{},"source":"c"},
              {"cell_type":"markdown","id":"bad id!","metadata":{},"source":"d"}
            ]"#,
            "rhai",
        );
        let report = import_ipynb(&nb, HOSTED).unwrap();
        let ids: Vec<&str> = report
            .document
            .cells
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["same", "imported-1", "imported-2", "imported-3"]);
    }

    #[test]
    fn rejects_other_nbformat_versions() {
        let nb = r#"{"nbformat":3,"nbformat_minor":0,"cells":[],"metadata":{}}"#;
        assert!(matches!(
            import_ipynb(nb, HOSTED),
            Err(DocError::Notebook(_))
        ));
    }

    #[test]
    fn export_then_import_keeps_document_content() {
        let mut doc = Document::new(DocId::new("doc-7").unwrap(), "Spring study");
        doc.metadata.insert("author".into(), "A".into());
        doc.cells.push(Cell {
            id: CellId::new("intro").unwrap(),
            body: CellBody::Markdown {
                source: "Line one\nLine two".into(),
            },
        });
        doc.cells.push(Cell {
            id: CellId::new("solve").unwrap(),
            body: CellBody::Code {
                language: "rhai".into(),
                source: "let k = 2.5;\nk".into(),
                outputs: vec![
                    Output::Text { text: "2.5".into() },
                    Output::Error {
                        name: "Warn".into(),
                        message: "close to zero".into(),
                    },
                ],
            },
        });

        let exported = export_ipynb(&doc).unwrap();
        let report = import_ipynb(&exported, HOSTED).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.document, doc);
    }

    #[test]
    fn a_plot_survives_export_and_import_as_svg() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"3\"/></svg>".to_string();
        let mut doc = Document::new(DocId::new("d").unwrap(), "plots");
        doc.cells.push(Cell {
            id: CellId::new("p").unwrap(),
            body: CellBody::Code {
                language: "rhai".into(),
                source: "show_plot(s)".into(),
                outputs: vec![Output::Svg { svg: svg.clone() }],
            },
        });
        let exported = export_ipynb(&doc).unwrap();
        let report = import_ipynb(&exported, HOSTED).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.document, doc);
    }

    #[test]
    fn exported_table_comes_back_as_text() {
        let mut doc = Document::new(DocId::new("d").unwrap(), "t");
        doc.cells.push(Cell {
            id: CellId::new("c").unwrap(),
            body: CellBody::Code {
                language: "rhai".into(),
                source: "table".into(),
                outputs: vec![Output::Table {
                    columns: vec!["t".into(), "x".into()],
                    rows: vec![vec!["0".into(), "1".into()]],
                }],
            },
        });
        let report = import_ipynb(&export_ipynb(&doc).unwrap(), HOSTED).unwrap();
        match &report.document.cells[0].body {
            CellBody::Code { outputs, .. } => assert_eq!(
                outputs,
                &vec![Output::Text {
                    text: "t\tx\n0\t1".into()
                }]
            ),
            other => panic!("expected code cell, got {other:?}"),
        }
    }

    #[test]
    fn export_is_deterministic() {
        let mut doc = Document::new(DocId::new("d").unwrap(), "t");
        doc.metadata.insert("b".into(), "2".into());
        doc.metadata.insert("a".into(), "1".into());
        assert_eq!(export_ipynb(&doc).unwrap(), export_ipynb(&doc).unwrap());
    }

    #[test]
    fn export_rejects_cell_ids_too_long_for_nbformat() {
        let mut doc = Document::new(DocId::new("d").unwrap(), "t");
        doc.cells.push(Cell {
            id: CellId::new("x".repeat(65)).unwrap(),
            body: CellBody::Markdown {
                source: String::new(),
            },
        });
        assert!(export_ipynb(&doc).is_err());
    }
}
