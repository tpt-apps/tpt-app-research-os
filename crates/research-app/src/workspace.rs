//! The application's model: one open project, its documents, and running them.
//! Nothing here draws anything, so the same code serves the window and the tests.

use std::path::Path;

use research_compute::{Control, Limits, RunSummary, Session};
use research_core::{DocId, ProjectId, ProjectManifest};
use research_doc::Document;
use research_store::{DocumentSummary, ProjectStore, Run};

/// Errors from workspace operations.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Store(#[from] research_store::StoreError),
    #[error(transparent)]
    Compute(#[from] research_compute::ComputeError),
    #[error(transparent)]
    Core(#[from] research_core::Error),
}

pub type Result<T> = std::result::Result<T, WorkspaceError>;

/// An open project together with the compute session its documents share.
pub struct Workspace {
    store: ProjectStore,
    session: Session,
}

impl Workspace {
    /// Create a new project in `dir`.
    pub fn create(dir: &Path, project_id: &str, name: &str, created_unix: u64) -> Result<Self> {
        let manifest = ProjectManifest::new(ProjectId::new(project_id)?, name, created_unix)?;
        let store = ProjectStore::create(dir, &manifest)?;
        Ok(Self::with_store(store)?)
    }

    /// Open an existing project in `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        Ok(Self::with_store(ProjectStore::open(dir)?)?)
    }

    fn with_store(
        store: ProjectStore,
    ) -> std::result::Result<Self, research_compute::ComputeError> {
        // Seed 0 is the default. A document that needs another seed sets it in its own cells.
        let session = Session::new(0, Limits::default())?;
        Ok(Self { store, session })
    }

    pub fn project_name(&self) -> &str {
        &self.store.manifest().name
    }

    /// Every document in the project, ordered by title.
    pub fn documents(&self) -> Result<Vec<DocumentSummary>> {
        Ok(self.store.list_documents()?)
    }

    /// Load a document, or `None` if it does not exist.
    pub fn document(&self, id: &str) -> Result<Option<Document>> {
        Ok(self.store.load_document(&DocId::new(id)?)?)
    }

    /// Create an empty document and save it.
    pub fn new_document(&self, id: &str, title: &str) -> Result<Document> {
        let doc = Document::new(DocId::new(id)?, title);
        self.store.save_document(&doc)?;
        Ok(doc)
    }

    /// Save a document, replacing the stored copy.
    pub fn save_document(&self, doc: &Document) -> Result<()> {
        Ok(self.store.save_document(doc)?)
    }

    /// Run the document's stale code cells, record them, and save the document with
    /// its new outputs. Cells that have not changed keep their outputs and are not
    /// recorded again.
    pub fn run_document(&mut self, doc: &mut Document, control: &Control) -> Result<RunSummary> {
        let prefix = doc.id.as_str().to_string();
        let summary = self.session.run_document(
            doc,
            control,
            Some(research_compute::Recording {
                store: &self.store,
                run_prefix: &prefix,
            }),
        )?;
        self.store.save_document(doc)?;
        Ok(summary)
    }

    /// Every recorded run, oldest first.
    pub fn runs(&self) -> Result<Vec<Run>> {
        Ok(self.store.list_runs()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use research_core::CellId;
    use research_doc::{Cell, CellBody, Output};

    fn code(id: &str, source: &str) -> Cell {
        Cell {
            id: CellId::new(id).unwrap(),
            body: CellBody::Code {
                language: research_compute::LANGUAGE.into(),
                source: source.into(),
                outputs: Vec::new(),
            },
        }
    }

    #[test]
    fn a_document_runs_records_and_survives_reopening() {
        let work = tempfile::tempdir().unwrap();
        let dir = work.path().join("study");
        {
            let mut ws = Workspace::create(&dir, "study-1", "Study", 1).unwrap();
            let mut doc = ws.new_document("doc-a", "Oscillator").unwrap();
            doc.cells.push(code("k", "let k = 2.5;"));
            doc.cells.push(code("e", "k * 4.0"));
            let summary = ws.run_document(&mut doc, &Control::new()).unwrap();
            assert_eq!(summary.ran.len(), 2);
        }

        let ws = Workspace::open(&dir).unwrap();
        assert_eq!(ws.project_name(), "Study");
        let titles: Vec<String> = ws
            .documents()
            .unwrap()
            .into_iter()
            .map(|d| d.title)
            .collect();
        assert_eq!(titles, ["Oscillator"]);

        let doc = ws.document("doc-a").unwrap().unwrap();
        let CellBody::Code { outputs, .. } = &doc.cells[1].body else {
            panic!("expected a code cell");
        };
        assert_eq!(
            outputs,
            &vec![Output::Text {
                text: "10.0".into()
            }]
        );
        assert_eq!(ws.runs().unwrap().len(), 2);
    }

    #[test]
    fn rerunning_an_unchanged_document_records_nothing_new() {
        let work = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(&work.path().join("p"), "p", "P", 1).unwrap();
        let mut doc = ws.new_document("d", "D").unwrap();
        doc.cells.push(code("a", "1 + 1"));
        ws.run_document(&mut doc, &Control::new()).unwrap();
        let again = ws.run_document(&mut doc, &Control::new()).unwrap();
        assert!(again.ran.is_empty());
        assert_eq!(ws.runs().unwrap().len(), 1);
    }

    #[test]
    fn bad_ids_are_reported_not_panicked() {
        let work = tempfile::tempdir().unwrap();
        let ws = Workspace::create(&work.path().join("p"), "p", "P", 1).unwrap();
        assert!(ws.new_document("../escape", "x").is_err());
        assert!(ws.document("has space").is_err());
    }
}
