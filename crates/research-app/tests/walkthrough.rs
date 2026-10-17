//! Success-criteria walk-through (spec §9): create a project, run a workflow, store
//! the results, pack it, and open it on "another machine" (a different directory),
//! where the runs and outputs are still there.

use research_app::Workspace;
use research_compute::{Control, LANGUAGE};
use research_core::CellId;
use research_doc::{Cell, CellBody, Output};
use research_package::{generate_signing_key, pack_project, unpack_project};

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

#[test]
fn a_project_packed_and_opened_elsewhere_keeps_its_runs_and_outputs() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("study");
    let key = generate_signing_key().unwrap();

    let runs_before = {
        let mut ws = Workspace::create(&source, "study-1", "Study", 1).unwrap();
        let mut doc = ws.new_document("doc-a", "Oscillator").unwrap();
        doc.cells.push(code("k", "let k = 2.5;"));
        doc.cells.push(code("e", "k * 4.0"));
        ws.run_document(&mut doc, &Control::new()).unwrap();
        ws.save_document(&doc).unwrap();
        ws.runs().unwrap().len()
    };
    assert_eq!(runs_before, 2);

    let packed = pack_project(&source, "2026-10-17T00:00:00Z", &key).unwrap();
    let other_machine = work.path().join("other-machine");
    unpack_project(&packed, &key.verifying_key(), &other_machine).unwrap();

    let ws = Workspace::open(&other_machine).unwrap();
    assert_eq!(ws.project_name(), "Study");
    assert_eq!(ws.runs().unwrap().len(), runs_before);
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
}
