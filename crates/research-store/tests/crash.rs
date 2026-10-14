//! Crash safety: a process that dies while writing must leave a project that
//! reopens, with every stored record intact.
//!
//! The test runs itself as a child process. The child writes runs and then aborts
//! part way through. The parent reopens the project and checks it.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use research_core::{ProjectId, ProjectManifest, RunId};
use research_store::{ProjectStore, Run};

const CHILD_DIR: &str = "RESEARCH_STORE_CRASH_CHILD_DIR";
const ABORT_AFTER: u64 = 150;
const TEST_NAME: &str = "project_survives_an_abort_mid_write";

fn run(i: u64) -> Run {
    Run {
        id: RunId::new(format!("run-{i:05}")).unwrap(),
        label: format!("run {i}"),
        inputs: BTreeMap::from([("step".to_string(), i.to_string())]),
        code_hash: "h".into(),
        environment: BTreeMap::new(),
        started_unix: i,
        finished_unix: i + 1,
    }
}

/// Runs in the child process only. Writes runs, then aborts.
fn write_until_abort(dir: &Path) -> ! {
    let store = ProjectStore::open(dir).expect("child opens the project");
    for i in 0..10_000 {
        store.save_run(&run(i)).expect("child writes a run");
        if i == ABORT_AFTER {
            std::process::abort();
        }
    }
    unreachable!("the child aborts before finishing");
}

#[test]
fn project_survives_an_abort_mid_write() {
    if let Ok(dir) = std::env::var(CHILD_DIR) {
        write_until_abort(Path::new(&dir));
    }

    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("project");
    {
        let manifest = ProjectManifest::new(ProjectId::new("crash").unwrap(), "Crash", 1).unwrap();
        ProjectStore::create(&root, &manifest).unwrap();
    }

    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST_NAME, "--nocapture", "--test-threads", "1"])
        .env(CHILD_DIR, &root)
        .status()
        .expect("child process starts");
    assert!(!status.success(), "the child was meant to abort");

    let store = ProjectStore::open(&root).expect("project reopens after the abort");
    let runs = store.list_runs().expect("every stored run parses");
    eprintln!("runs recovered after the abort: {}", runs.len());
    assert!(
        runs.len() <= ABORT_AFTER as usize + 1,
        "more runs than were written"
    );
    for stored in &runs {
        assert!(
            stored.label.starts_with("run "),
            "unexpected label {:?}",
            stored.label
        );
    }

    store
        .save_run(&run(99_999))
        .expect("the project accepts writes after the abort");
    assert!(store
        .load_run(&RunId::new("run-99999").unwrap())
        .unwrap()
        .is_some());
}
