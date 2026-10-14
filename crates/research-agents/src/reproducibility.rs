//! Reproducibility check: re-run a document from scratch with the same seed, and
//! compare each code cell's new outputs with the stored ones.
//!
//! The check works on a copy, so the stored document is never changed and nothing is
//! written to a project. Through [`check_with_agent`] it also needs a capability that
//! lets the agent rerun that document.

use std::fmt;

use research_compute::{ComputeError, Control, Limits, Session};
use research_core::CellId;
use research_doc::{CellBody, Document, Output};
use tpt_primitives::capability::Grant;
use tpt_primitives::LogicalTime;

use crate::agent::{Agent, AgentError};

/// What the check found for one code cell.
#[derive(Debug, Clone, PartialEq)]
pub enum CellCheck {
    /// The fresh run gives exactly the stored outputs.
    Matches,
    /// The fresh run gives different outputs.
    Differs {
        stored: Vec<Output>,
        rerun: Vec<Output>,
    },
    /// The cell has no stored outputs, so there is nothing to compare with.
    NotStored,
}

/// The result for every code cell, in document order.
#[derive(Debug, Clone, PartialEq)]
pub struct ReproReport {
    pub cells: Vec<(CellId, CellCheck)>,
}

impl ReproReport {
    /// True if no cell's fresh outputs differ from the stored ones.
    /// Cells that were never run are not counted as differences.
    pub fn is_reproducible(&self) -> bool {
        self.cells
            .iter()
            .all(|(_, check)| !matches!(check, CellCheck::Differs { .. }))
    }
}

/// Errors from the check.
#[derive(Debug)]
pub enum CheckError {
    /// The fresh run could not start, for example because a limit is invalid.
    Compute(ComputeError),
    /// The agent is not allowed to rerun this document.
    Agent(AgentError),
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compute(e) => write!(f, "{e}"),
            Self::Agent(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CheckError {}

impl From<ComputeError> for CheckError {
    fn from(e: ComputeError) -> Self {
        Self::Compute(e)
    }
}

impl From<AgentError> for CheckError {
    fn from(e: AgentError) -> Self {
        Self::Agent(e)
    }
}

/// The capability an agent needs to rerun document `doc_id`.
pub fn rerun_grant(doc_id: &str) -> Grant {
    Grant::new(format!("document:{doc_id}"), "rerun").expect("document ids are non-empty")
}

/// Re-run `doc` from scratch with `seed` and compare with its stored outputs.
pub fn check_document(
    doc: &Document,
    seed: u64,
    limits: Limits,
) -> Result<ReproReport, CheckError> {
    let mut rerun = doc.clone();
    for cell in &mut rerun.cells {
        if let CellBody::Code { outputs, .. } = &mut cell.body {
            outputs.clear();
        }
    }
    let mut session = Session::new(seed, limits)?;
    session.run_document(&mut rerun, &Control::new(), None)?;

    let mut cells = Vec::new();
    for (stored, fresh) in doc.cells.iter().zip(&rerun.cells) {
        let (
            CellBody::Code {
                outputs: before, ..
            },
            CellBody::Code { outputs: after, .. },
        ) = (&stored.body, &fresh.body)
        else {
            continue;
        };
        let check = if before.is_empty() {
            CellCheck::NotStored
        } else if before == after {
            CellCheck::Matches
        } else {
            CellCheck::Differs {
                stored: before.clone(),
                rerun: after.clone(),
            }
        };
        cells.push((stored.id.clone(), check));
    }
    Ok(ReproReport { cells })
}

/// As [`check_document`], but only if `agent` holds [`rerun_grant`] for `doc_id` at
/// logical time `at`. The attempt is logged either way.
pub fn check_with_agent(
    agent: &mut Agent,
    doc_id: &str,
    doc: &Document,
    seed: u64,
    limits: Limits,
    at: LogicalTime,
) -> Result<ReproReport, CheckError> {
    agent.perform(rerun_grant(doc_id), at)?;
    check_document(doc, seed, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use research_core::DocId;
    use research_doc::Cell;
    use tpt_primitives::capability::{AuthorityDomain, Capability, Principal};

    fn code(id: &str, source: &str) -> Cell {
        Cell {
            id: CellId::new(id).unwrap(),
            body: CellBody::Code {
                language: "rhai".into(),
                source: source.into(),
                outputs: Vec::new(),
            },
        }
    }

    /// A document run once with `seed`, so its cells hold stored outputs.
    fn run_once(seed: u64, cells: Vec<Cell>) -> Document {
        let mut doc = Document::new(DocId::new("doc").unwrap(), "Doc");
        doc.cells = cells;
        Session::new(seed, Limits::default())
            .unwrap()
            .run_document(&mut doc, &Control::new(), None)
            .unwrap();
        doc
    }

    fn cell_check(report: &ReproReport, id: &str) -> CellCheck {
        report
            .cells
            .iter()
            .find(|(c, _)| c.as_str() == id)
            .map(|(_, check)| check.clone())
            .unwrap()
    }

    #[test]
    fn a_document_reproduces_with_the_same_seed() {
        let doc = run_once(
            5,
            vec![code("a", "let k = 2.5;"), code("b", "k * uniform(1)")],
        );
        let report = check_document(&doc, 5, Limits::default()).unwrap();
        assert!(report.is_reproducible());
        assert_eq!(cell_check(&report, "b"), CellCheck::Matches);
    }

    #[test]
    fn a_different_seed_is_caught_as_a_difference() {
        let doc = run_once(5, vec![code("u", "uniform(3)")]);
        let report = check_document(&doc, 6, Limits::default()).unwrap();
        assert!(!report.is_reproducible());
        assert!(matches!(
            cell_check(&report, "u"),
            CellCheck::Differs { .. }
        ));
    }

    #[test]
    fn tampered_stored_outputs_are_caught() {
        let mut doc = run_once(1, vec![code("a", "2 + 2")]);
        if let CellBody::Code { outputs, .. } = &mut doc.cells[0].body {
            *outputs = vec![Output::Text { text: "5".into() }];
        }
        let report = check_document(&doc, 1, Limits::default()).unwrap();
        match cell_check(&report, "a") {
            CellCheck::Differs { stored, rerun } => {
                assert_eq!(stored, vec![Output::Text { text: "5".into() }]);
                assert_eq!(rerun, vec![Output::Text { text: "4".into() }]);
            }
            other => panic!("expected a difference, got {other:?}"),
        }
    }

    #[test]
    fn cells_never_run_are_reported_not_stored() {
        let doc = Document {
            format_version: 1,
            id: DocId::new("doc").unwrap(),
            title: "Doc".into(),
            metadata: Default::default(),
            cells: vec![code("a", "1 + 1")],
        };
        let report = check_document(&doc, 1, Limits::default()).unwrap();
        assert_eq!(cell_check(&report, "a"), CellCheck::NotStored);
        assert!(report.is_reproducible());
    }

    #[test]
    fn checking_leaves_the_stored_document_unchanged() {
        let doc = run_once(2, vec![code("a", "7 * 6")]);
        let before = doc.clone();
        check_document(&doc, 2, Limits::default()).unwrap();
        assert_eq!(doc, before);
    }

    fn agent_with(grants: Vec<Grant>) -> Agent {
        let mut agent = Agent::new("repro-check").unwrap();
        let cap = Capability::mint(
            AuthorityDomain::new("research").unwrap(),
            Principal::new("repro-check").unwrap(),
            grants,
            false,
        )
        .unwrap();
        agent.grant(cap).unwrap();
        agent.start();
        agent
    }

    #[test]
    fn an_agent_without_the_rerun_grant_cannot_check() {
        let doc = run_once(1, vec![code("a", "1")]);
        let mut agent = agent_with(vec![Grant::new("document:other", "rerun").unwrap()]);
        let r = check_with_agent(
            &mut agent,
            "doc",
            &doc,
            1,
            Limits::default(),
            LogicalTime::new(1),
        );
        assert!(matches!(r, Err(CheckError::Agent(_))));
        assert_eq!(agent.log().len(), 1);
    }

    #[test]
    fn an_agent_with_the_rerun_grant_can_check() {
        let doc = run_once(1, vec![code("a", "1")]);
        let mut agent = agent_with(vec![rerun_grant("doc")]);
        let report = check_with_agent(
            &mut agent,
            "doc",
            &doc,
            1,
            Limits::default(),
            LogicalTime::new(1),
        )
        .unwrap();
        assert!(report.is_reproducible());
    }
}
