//! The document model and its JSON form. See `docs/DOCUMENT_FORMAT.md`.

use std::collections::{BTreeMap, HashSet};

use research_core::{CellId, DocId};
use serde::{Deserialize, Serialize};

use crate::error::{DocError, Result};

/// Current document format version.
pub const FORMAT_VERSION: u32 = 1;

/// A computational document: narrative and code cells, in reading order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format_version: u32,
    pub id: DocId,
    pub title: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    pub cells: Vec<Cell>,
}

/// One cell: its stable id and what it contains.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub id: CellId,
    #[serde(flatten)]
    pub body: CellBody,
}

/// The content of a cell, tagged by `type` in the JSON form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CellBody {
    Markdown {
        source: String,
    },
    Code {
        language: String,
        source: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        outputs: Vec<Output>,
    },
}

/// One output of a code cell, stored so the document reads without re-running.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Output {
    Text {
        text: String,
    },
    Error {
        name: String,
        message: String,
    },
    /// Each row has one entry per column.
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

impl Document {
    /// An empty document with the current format version.
    pub fn new(id: DocId, title: impl Into<String>) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            id,
            title: title.into(),
            metadata: BTreeMap::new(),
            cells: Vec::new(),
        }
    }

    /// The cell with `id`, if present.
    pub fn cell(&self, id: &CellId) -> Option<&Cell> {
        self.cells.iter().find(|c| &c.id == id)
    }

    /// Check the rules that serde cannot: version, unique cell ids, table shape.
    pub fn validate(&self) -> Result<()> {
        if self.format_version > FORMAT_VERSION {
            return Err(DocError::UnsupportedFormatVersion {
                found: self.format_version,
                supported: FORMAT_VERSION,
            });
        }
        let mut seen = HashSet::new();
        for cell in &self.cells {
            if !seen.insert(cell.id.as_str()) {
                return Err(DocError::Invalid(format!("duplicate cell id {}", cell.id)));
            }
            if let CellBody::Code { outputs, .. } = &cell.body {
                for output in outputs {
                    validate_output(&cell.id, output)?;
                }
            }
        }
        Ok(())
    }

    /// Parse and validate a document from JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        let doc: Self = serde_json::from_str(json)?;
        doc.validate()?;
        Ok(doc)
    }

    /// Serialize as pretty-printed JSON with a trailing newline. The output is
    /// the same for equal documents, so it diffs cleanly.
    pub fn to_json(&self) -> Result<String> {
        self.validate()?;
        let mut out = serde_json::to_string_pretty(self)?;
        out.push('\n');
        Ok(out)
    }
}

fn validate_output(cell: &CellId, output: &Output) -> Result<()> {
    if let Output::Table { columns, rows } = output {
        if columns.is_empty() {
            return Err(DocError::Invalid(format!(
                "cell {cell}: table has no columns"
            )));
        }
        if rows.iter().any(|row| row.len() != columns.len()) {
            return Err(DocError::Invalid(format!(
                "cell {cell}: table row width does not match its columns"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_id(s: &str) -> CellId {
        CellId::new(s).unwrap()
    }

    fn sample() -> Document {
        let mut doc = Document::new(DocId::new("doc-1").unwrap(), "Oscillator");
        doc.metadata.insert("author".into(), "A".into());
        doc.cells.push(Cell {
            id: cell_id("intro"),
            body: CellBody::Markdown {
                source: "Some *text*\nover two lines".into(),
            },
        });
        doc.cells.push(Cell {
            id: cell_id("solve"),
            body: CellBody::Code {
                language: "rhai".into(),
                source: "let k = 2.5;".into(),
                outputs: vec![
                    Output::Text {
                        text: "k = 2.5\n".into(),
                    },
                    Output::Table {
                        columns: vec!["t".into(), "x".into()],
                        rows: vec![vec!["0".into(), "1".into()]],
                    },
                    Output::Error {
                        name: "DivByZero".into(),
                        message: "x is zero".into(),
                    },
                ],
            },
        });
        doc
    }

    #[test]
    fn json_round_trip_is_exact_and_stable() {
        let doc = sample();
        let json = doc.to_json().unwrap();
        let back = Document::from_json(&json).unwrap();
        assert_eq!(back, doc);
        assert_eq!(back.to_json().unwrap(), json);
    }

    #[test]
    fn cell_order_is_preserved() {
        let json = sample().to_json().unwrap();
        let back = Document::from_json(&json).unwrap();
        let ids: Vec<&str> = back.cells.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["intro", "solve"]);
    }

    #[test]
    fn empty_metadata_is_omitted() {
        let doc = Document::new(DocId::new("d").unwrap(), "t");
        assert!(!doc.to_json().unwrap().contains("metadata"));
    }

    #[test]
    fn rejects_duplicate_cell_ids() {
        let mut doc = sample();
        doc.cells[1].id = cell_id("intro");
        assert!(matches!(doc.validate(), Err(DocError::Invalid(_))));
    }

    #[test]
    fn rejects_newer_format_version() {
        let mut doc = sample();
        doc.format_version = FORMAT_VERSION + 1;
        let json = serde_json::to_string(&doc).unwrap();
        assert!(matches!(
            Document::from_json(&json),
            Err(DocError::UnsupportedFormatVersion { .. })
        ));
    }

    #[test]
    fn rejects_ragged_table() {
        let mut doc = sample();
        if let CellBody::Code { outputs, .. } = &mut doc.cells[1].body {
            outputs[1] = Output::Table {
                columns: vec!["a".into(), "b".into()],
                rows: vec![vec!["only one".into()]],
            };
        }
        assert!(doc.validate().is_err());
    }

    #[test]
    fn rejects_invalid_ids_in_json() {
        let json = r#"{"format_version":1,"id":"../x","title":"t","cells":[]}"#;
        assert!(matches!(
            Document::from_json(json),
            Err(DocError::Serialization(_))
        ));
    }
}
