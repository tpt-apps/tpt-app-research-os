//! Storage for computational documents (see `research-doc`).
//!
//! Each document is stored as its JSON form. The title is kept in its own column so
//! listings do not parse every document.

use research_core::DocId;
use research_doc::Document;

use crate::error::Result;
use crate::project::{sql_text, ProjectStore};

/// What a listing shows about a stored document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSummary {
    pub id: DocId,
    pub title: String,
}

impl ProjectStore {
    /// Insert or replace a document. It is validated before it is written.
    pub fn save_document(&self, doc: &Document) -> Result<()> {
        let json = doc.to_json()?;
        let id = sql_text(doc.id.as_str());
        self.exec(&format!("DELETE FROM documents WHERE id = {id}"))?;
        self.exec(&format!(
            "INSERT INTO documents (id, title, doc) VALUES ({id}, {}, {})",
            sql_text(&doc.title),
            sql_text(&json)
        ))
    }

    /// Load a document by id, or `None` if it is not stored.
    pub fn load_document(&self, id: &DocId) -> Result<Option<Document>> {
        let rows = self.rows(&format!(
            "SELECT doc FROM documents WHERE id = {}",
            sql_text(id.as_str())
        ))?;
        match rows.first().and_then(|row| row.first()) {
            None => Ok(None),
            Some(json) => Ok(Some(Document::from_json(json)?)),
        }
    }

    /// Every stored document as id and title, ordered by title then id.
    pub fn list_documents(&self) -> Result<Vec<DocumentSummary>> {
        let mut summaries = self
            .rows("SELECT id, title FROM documents")?
            .into_iter()
            .map(|row| {
                let id = row.first().map(String::as_str).unwrap_or_default();
                Ok(DocumentSummary {
                    id: DocId::new(id)?,
                    title: row.get(1).cloned().unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        summaries.sort_by(|a, b| (&a.title, &a.id).cmp(&(&b.title, &b.id)));
        Ok(summaries)
    }

    /// Delete a document. Deleting one that does not exist is not an error.
    pub fn delete_document(&self, id: &DocId) -> Result<()> {
        self.exec(&format!(
            "DELETE FROM documents WHERE id = {}",
            sql_text(id.as_str())
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use research_core::{CellId, ProjectId, ProjectManifest};
    use research_doc::{Cell, CellBody, Output};

    fn open_temp() -> (tempfile::TempDir, ProjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let manifest = ProjectManifest::new(ProjectId::new("p").unwrap(), "P", 0).unwrap();
        let project = ProjectStore::create(&dir.path().join("p"), &manifest).unwrap();
        (dir, project)
    }

    fn doc(id: &str, title: &str) -> Document {
        let mut doc = Document::new(DocId::new(id).unwrap(), title);
        doc.cells.push(Cell {
            id: CellId::new("intro").unwrap(),
            body: CellBody::Markdown {
                source: "Hello".into(),
            },
        });
        doc.cells.push(Cell {
            id: CellId::new("solve").unwrap(),
            body: CellBody::Code {
                language: "rhai".into(),
                source: "1 + 1".into(),
                outputs: vec![Output::Text { text: "2".into() }],
            },
        });
        doc
    }

    #[test]
    fn document_round_trips_and_is_replaced_in_place() {
        let (_dir, project) = open_temp();
        let id = DocId::new("d1").unwrap();
        project.save_document(&doc("d1", "First")).unwrap();
        assert_eq!(
            project.load_document(&id).unwrap().unwrap(),
            doc("d1", "First")
        );

        project.save_document(&doc("d1", "Renamed")).unwrap();
        assert_eq!(
            project.load_document(&id).unwrap().unwrap().title,
            "Renamed"
        );
        assert_eq!(project.list_documents().unwrap().len(), 1);
    }

    #[test]
    fn listing_is_ordered_by_title() {
        let (_dir, project) = open_temp();
        project.save_document(&doc("b", "Beta")).unwrap();
        project.save_document(&doc("a", "Alpha")).unwrap();
        let titles: Vec<String> = project
            .list_documents()
            .unwrap()
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(titles, ["Alpha", "Beta"]);
    }

    #[test]
    fn invalid_documents_are_refused() {
        let (_dir, project) = open_temp();
        let mut bad = doc("d", "Bad");
        bad.cells[1].id = CellId::new("intro").unwrap();
        assert!(project.save_document(&bad).is_err());
        assert!(project
            .load_document(&DocId::new("d").unwrap())
            .unwrap()
            .is_none());
    }

    #[test]
    fn delete_removes_the_document() {
        let (_dir, project) = open_temp();
        let id = DocId::new("gone").unwrap();
        project.save_document(&doc("gone", "Gone")).unwrap();
        project.delete_document(&id).unwrap();
        assert!(project.load_document(&id).unwrap().is_none());
        project.delete_document(&id).unwrap();
    }

    #[test]
    fn titles_and_bodies_with_quotes_are_stored_safely() {
        let (_dir, project) = open_temp();
        let mut d = doc("q", "O'Hara's notes");
        d.cells[0].body = CellBody::Markdown {
            source: "it's fine".into(),
        };
        project.save_document(&d).unwrap();
        assert_eq!(project.load_document(&d.id).unwrap().unwrap(), d);
    }
}
