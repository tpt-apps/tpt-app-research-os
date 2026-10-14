//! TPT Research OS desktop window. Opens the project in the directory given on the
//! command line, or `research-project` in the current directory, creating it if it
//! does not exist yet.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use research_app::{apply, view, Editor, Msg, Workspace};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("research-project"));
    let workspace = if dir.join("manifest.json").exists() {
        Workspace::open(&dir)?
    } else {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        Workspace::create(&dir, "research-project", "Research project", now)?
    };

    let editor = Rc::new(RefCell::new(Editor::new(workspace)));
    let for_view = editor.clone();
    let build_ui = move || view(&for_view.borrow());
    let for_apply = editor.clone();
    let dispatch = move |msg: Msg| apply(&mut for_apply.borrow_mut(), msg);

    tpt_appfront_canvas::run_native("TPT Research OS", build_ui, dispatch)?;
    Ok(())
}
