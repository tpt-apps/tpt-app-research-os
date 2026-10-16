//! Editor state for one open document: what the window shows and what its buttons
//! do. Everything here is plain Rust, so the tests run without a display.

use std::fmt;

use research_agents::reproducibility::rerun_grant;
use research_agents::{check_with_agent, ActionRecord, Agent, CellCheck, CheckError, ReproReport};
use research_compute::{Control, Limits, RunSummary, LANGUAGE};
use research_core::{CellId, RunId};
use research_doc::{Cell, CellBody, Document};
use research_store::{DocumentSummary, Run, RunComparison};
use tpt_primitives::capability::{AuthorityDomain, Capability};
use tpt_primitives::LogicalTime;

/// Name of the agent that performs reproducibility checks.
const CHECKER: &str = "reproducibility-checker";
/// Authority domain the checker's capabilities are minted in.
const DOMAIN: &str = "research";
/// Seed the editor uses for runs, so a check reproduces what the window ran.
const SEED: u64 = 0;

use crate::workspace::{Workspace, WorkspaceError};

/// What an added cell is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Code,
    Markdown,
}

/// Errors from editor actions.
#[derive(Debug)]
pub enum EditorError {
    /// The underlying workspace refused the action.
    Workspace(WorkspaceError),
    /// The action needs an open document.
    NoDocumentOpen,
    /// No stored document has this id.
    DocumentMissing(String),
    /// The cell index is past the end of the document.
    NoSuchCell(usize),
    /// Comparing needs at least two selected runs.
    NotEnoughRuns,
    /// A reproducibility check needs agents switched on in settings.
    AgentsOff,
    /// The agent or its capability could not be set up.
    Agent(String),
    /// The check itself failed.
    Check(CheckError),
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace(e) => write!(f, "{e}"),
            Self::NoDocumentOpen => write!(f, "open a document first"),
            Self::DocumentMissing(id) => write!(f, "no document called {id}"),
            Self::NoSuchCell(i) => write!(f, "there is no cell {i}"),
            Self::NotEnoughRuns => write!(f, "select at least two runs to compare"),
            Self::AgentsOff => write!(f, "agents are off; turn them on in settings first"),
            Self::Agent(reason) => write!(f, "agent setup failed: {reason}"),
            Self::Check(e) => write!(f, "reproducibility check failed: {e}"),
        }
    }
}

impl std::error::Error for EditorError {}

impl From<WorkspaceError> for EditorError {
    fn from(e: WorkspaceError) -> Self {
        Self::Workspace(e)
    }
}

pub type Result<T> = std::result::Result<T, EditorError>;

/// The open document, its status line, and the workspace it lives in.
pub struct Editor {
    workspace: Workspace,
    document: Option<Document>,
    status: String,
    /// Runs ticked for comparison, in the order they were ticked.
    selected_runs: Vec<RunId>,
    comparison: Option<RunComparison>,
    agents_enabled: bool,
    /// Every action any checker agent attempted, in order.
    agent_log: Vec<ActionRecord>,
    /// Logical time for agent actions; advances once per check.
    clock: u64,
}

impl Editor {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            document: None,
            status: "Open or create a document to start.".into(),
            selected_runs: Vec::new(),
            comparison: None,
            agents_enabled: false,
            agent_log: Vec::new(),
            clock: 0,
        }
    }

    /// Whether agents may run. Off until the user turns them on.
    pub fn agents_enabled(&self) -> bool {
        self.agents_enabled
    }

    pub fn set_agents_enabled(&mut self, on: bool) {
        self.agents_enabled = on;
        self.status = format!("Agents {}.", if on { "on" } else { "off" });
    }

    /// Every action attempted by a checker agent, oldest first.
    pub fn agent_log(&self) -> &[ActionRecord] {
        &self.agent_log
    }

    /// Re-run the open document from scratch and compare with its stored outputs.
    /// Each check uses a fresh agent that holds only the rerun grant for this
    /// document, and the agent is stopped afterwards.
    pub fn check_reproducibility(&mut self) -> Result<ReproReport> {
        if !self.agents_enabled {
            return Err(EditorError::AgentsOff);
        }
        let doc = self.document.as_ref().ok_or(EditorError::NoDocumentOpen)?;
        let doc_id = doc.id.to_string();

        let mut agent = Agent::new(CHECKER).map_err(|e| EditorError::Agent(e.to_string()))?;
        let domain = AuthorityDomain::new(DOMAIN).map_err(|e| EditorError::Agent(e.to_string()))?;
        let capability = Capability::mint(
            domain,
            agent.principal().clone(),
            [rerun_grant(&doc_id)],
            false,
        )
        .map_err(|e| EditorError::Agent(e.to_string()))?;
        agent
            .grant(capability)
            .map_err(|e| EditorError::Agent(e.to_string()))?;

        agent.start();
        self.clock += 1;
        let result = check_with_agent(
            &mut agent,
            &doc_id,
            doc,
            SEED,
            Limits::default(),
            LogicalTime::new(self.clock),
        );
        agent.stop();
        self.agent_log.extend(agent.log().iter().cloned());

        let report = result.map_err(EditorError::Check)?;
        self.status = reproducibility_status(&report);
        Ok(report)
    }

    pub fn project_name(&self) -> &str {
        self.workspace.project_name()
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }

    /// Every document in the project, for the list on the left.
    pub fn documents(&self) -> Result<Vec<DocumentSummary>> {
        Ok(self.workspace.documents()?)
    }

    /// Make the stored document `id` the open one.
    pub fn open(&mut self, id: &str) -> Result<()> {
        let doc = self
            .workspace
            .document(id)?
            .ok_or_else(|| EditorError::DocumentMissing(id.to_string()))?;
        self.status = format!("Opened {}.", doc.title);
        self.document = Some(doc);
        Ok(())
    }

    /// Create an empty document, save it, and open it.
    pub fn create(&mut self, id: &str, title: &str) -> Result<()> {
        let doc = self.workspace.new_document(id, title)?;
        self.status = format!("Created {}.", doc.title);
        self.document = Some(doc);
        Ok(())
    }

    /// Replace the source of cell `index`. Nothing runs until the document does.
    pub fn set_source(&mut self, index: usize, source: &str) -> Result<()> {
        let doc = self.document_mut()?;
        let cell = doc
            .cells
            .get_mut(index)
            .ok_or(EditorError::NoSuchCell(index))?;
        match &mut cell.body {
            CellBody::Code { source: s, .. } | CellBody::Markdown { source: s } => {
                *s = source.to_string();
            }
        }
        Ok(())
    }

    /// Append a cell of `kind` with a fresh id, and return its index.
    pub fn add_cell(&mut self, kind: CellKind) -> Result<usize> {
        let doc = self.document_mut()?;
        let id = fresh_cell_id(doc);
        let body = match kind {
            CellKind::Code => CellBody::Code {
                language: LANGUAGE.into(),
                source: String::new(),
                outputs: Vec::new(),
            },
            CellKind::Markdown => CellBody::Markdown {
                source: String::new(),
            },
        };
        doc.cells.push(Cell { id, body });
        Ok(doc.cells.len() - 1)
    }

    /// Every recorded run, oldest first, for the runs list.
    pub fn runs(&self) -> Result<Vec<Run>> {
        Ok(self.workspace.runs()?)
    }

    pub fn selected_runs(&self) -> &[RunId] {
        &self.selected_runs
    }

    /// The last comparison made, if it is still current.
    pub fn comparison(&self) -> Option<&RunComparison> {
        self.comparison.as_ref()
    }

    /// Tick or untick run `id` for comparison. Changing the selection drops the old
    /// comparison, since it no longer matches what is ticked.
    pub fn toggle_run(&mut self, id: &str) -> Result<()> {
        let id = RunId::new(id).map_err(|e| EditorError::Workspace(e.into()))?;
        match self.selected_runs.iter().position(|r| r == &id) {
            Some(i) => {
                self.selected_runs.remove(i);
            }
            None => self.selected_runs.push(id),
        }
        self.comparison = None;
        Ok(())
    }

    /// Compare the ticked runs side by side and show the result.
    pub fn compare_selected(&mut self) -> Result<()> {
        if self.selected_runs.len() < 2 {
            return Err(EditorError::NotEnoughRuns);
        }
        let cmp = self.workspace.compare_runs(&self.selected_runs)?;
        self.status = format!("Compared {} runs.", cmp.runs.len());
        self.comparison = Some(cmp);
        Ok(())
    }

    /// Close the comparison and untick every run.
    pub fn clear_comparison(&mut self) {
        self.selected_runs.clear();
        self.comparison = None;
    }

    /// Replace the status line, for errors the window needs to show.
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// Create an empty document with the first free `doc-N` id, titled `Untitled N`.
    pub fn create_untitled(&mut self) -> Result<()> {
        let existing: Vec<String> = self
            .documents()?
            .into_iter()
            .map(|d| d.id.to_string())
            .collect();
        let mut n = existing.len() + 1;
        while existing.iter().any(|id| id == &format!("doc-{n}")) {
            n += 1;
        }
        self.create(&format!("doc-{n}"), &format!("Untitled {n}"))
    }

    /// Save the open document.
    pub fn save(&mut self) -> Result<()> {
        let doc = self.document.as_ref().ok_or(EditorError::NoDocumentOpen)?;
        self.workspace.save_document(doc)?;
        self.status = format!("Saved {}.", doc.title);
        Ok(())
    }

    /// Run the open document's stale cells and save it. The summary is also turned
    /// into the status line.
    pub fn run(&mut self) -> Result<RunSummary> {
        let doc = self.document.as_mut().ok_or(EditorError::NoDocumentOpen)?;
        let summary = self.workspace.run_document(doc, &Control::new())?;
        self.status = if summary.cancelled {
            "Run cancelled.".into()
        } else {
            format!(
                "Ran {} cell(s); kept {} unchanged.",
                summary.ran.len(),
                summary.reused.len()
            )
        };
        Ok(summary)
    }

    fn document_mut(&mut self) -> Result<&mut Document> {
        self.document.as_mut().ok_or(EditorError::NoDocumentOpen)
    }
}

/// One line for the status bar: which cells match, or which differ.
fn reproducibility_status(report: &ReproReport) -> String {
    let differing: Vec<&str> = report
        .cells
        .iter()
        .filter(|(_, check)| matches!(check, CellCheck::Differs { .. }))
        .map(|(id, _)| id.as_str())
        .collect();
    if differing.is_empty() {
        let matching = report
            .cells
            .iter()
            .filter(|(_, check)| *check == CellCheck::Matches)
            .count();
        let never_run = report
            .cells
            .iter()
            .filter(|(_, check)| *check == CellCheck::NotStored)
            .count();
        format!("Reproducible: {matching} cell(s) match, {never_run} never run.")
    } else {
        format!(
            "Not reproducible: {} cell(s) differ ({}).",
            differing.len(),
            differing.join(", ")
        )
    }
}

/// An id no cell in `doc` uses yet: `cell-N` for the first free N.
fn fresh_cell_id(doc: &Document) -> CellId {
    let mut n = doc.cells.len() + 1;
    loop {
        let candidate = format!("cell-{n}");
        if doc.cells.iter().all(|c| c.id.as_str() != candidate) {
            return CellId::new(candidate).expect("generated ids are valid");
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use research_doc::Output;

    fn editor() -> (tempfile::TempDir, Editor) {
        let work = tempfile::tempdir().unwrap();
        let ws = Workspace::create(&work.path().join("p"), "p", "Project", 1).unwrap();
        (work, Editor::new(ws))
    }

    fn source(doc: &Document, index: usize) -> String {
        match &doc.cells[index].body {
            CellBody::Code { source, .. } | CellBody::Markdown { source } => source.clone(),
        }
    }

    #[test]
    fn actions_need_an_open_document() {
        let (_work, mut ed) = editor();
        assert!(matches!(ed.save(), Err(EditorError::NoDocumentOpen)));
        assert!(matches!(
            ed.add_cell(CellKind::Code),
            Err(EditorError::NoDocumentOpen)
        ));
        assert!(matches!(ed.run(), Err(EditorError::NoDocumentOpen)));
    }

    #[test]
    fn opening_a_missing_document_is_reported() {
        let (_work, mut ed) = editor();
        assert!(matches!(
            ed.open("nope"),
            Err(EditorError::DocumentMissing(_))
        ));
    }

    #[test]
    fn cells_are_added_with_unique_ids_and_edited() {
        let (_work, mut ed) = editor();
        ed.create("d", "Draft").unwrap();
        let a = ed.add_cell(CellKind::Markdown).unwrap();
        let b = ed.add_cell(CellKind::Code).unwrap();
        ed.set_source(a, "# Notes").unwrap();
        ed.set_source(b, "1 + 1").unwrap();

        let doc = ed.document().unwrap();
        let ids: Vec<&str> = doc.cells.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["cell-1", "cell-2"]);
        assert_eq!(source(doc, a), "# Notes");
        assert_eq!(source(doc, b), "1 + 1");
        assert!(matches!(
            ed.set_source(9, "x"),
            Err(EditorError::NoSuchCell(9))
        ));
    }

    #[test]
    fn edits_survive_save_and_reopen_and_run_produces_outputs() {
        let (work, mut ed) = editor();
        ed.create("d", "Draft").unwrap();
        let i = ed.add_cell(CellKind::Code).unwrap();
        ed.set_source(i, "6 * 7").unwrap();
        let summary = ed.run().unwrap();
        assert_eq!(summary.ran.len(), 1);
        assert!(ed.status().contains("Ran 1 cell"));

        drop(ed);
        let stored = Workspace::open(&work.path().join("p")).unwrap();
        let mut again = Editor::new(stored);
        again.open("d").unwrap();
        let doc = again.document().unwrap();
        let CellBody::Code { outputs, .. } = &doc.cells[i].body else {
            panic!("expected code cell");
        };
        assert_eq!(outputs, &vec![Output::Text { text: "42".into() }]);
    }

    #[test]
    fn an_edit_after_a_run_is_run_again_and_an_unchanged_run_is_not() {
        let (_work, mut ed) = editor();
        ed.create("d", "Draft").unwrap();
        let i = ed.add_cell(CellKind::Code).unwrap();
        ed.set_source(i, "2 + 2").unwrap();
        ed.run().unwrap();
        assert!(ed.run().unwrap().ran.is_empty());

        ed.set_source(i, "2 + 3").unwrap();
        assert_eq!(ed.run().unwrap().ran.len(), 1);
    }

    #[test]
    fn a_reproducibility_check_needs_agents_switched_on() {
        let (_work, mut ed) = editor();
        ed.create("d", "Draft").unwrap();
        assert!(matches!(
            ed.check_reproducibility(),
            Err(EditorError::AgentsOff)
        ));
        assert!(ed.agent_log().is_empty());
    }

    #[test]
    fn a_checked_document_reproduces_and_each_attempt_is_logged() {
        let (_work, mut ed) = editor();
        ed.create("d", "Draft").unwrap();
        let i = ed.add_cell(CellKind::Code).unwrap();
        ed.set_source(i, "6 * 7").unwrap();
        ed.run().unwrap();

        ed.set_agents_enabled(true);
        let report = ed.check_reproducibility().unwrap();
        assert!(report.is_reproducible());
        assert_eq!(ed.status(), "Reproducible: 1 cell(s) match, 0 never run.");
        assert_eq!(ed.agent_log().len(), 1);
        assert_eq!(ed.agent_log()[0].denial, None);
    }

    #[test]
    fn a_changed_stored_output_is_reported_as_not_reproducible() {
        let (_work, mut ed) = editor();
        ed.create("d", "Draft").unwrap();
        let i = ed.add_cell(CellKind::Code).unwrap();
        ed.set_source(i, "6 * 7").unwrap();
        ed.run().unwrap();
        if let CellBody::Code { outputs, .. } = &mut ed.document_mut().unwrap().cells[i].body {
            *outputs = vec![Output::Text { text: "0".into() }];
        }

        ed.set_agents_enabled(true);
        let report = ed.check_reproducibility().unwrap();
        assert!(!report.is_reproducible());
        assert!(ed
            .status()
            .starts_with("Not reproducible: 1 cell(s) differ"));
    }
}
