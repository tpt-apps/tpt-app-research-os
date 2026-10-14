//! The editor window's layout. `view` turns the editor state into an AppFront tree,
//! and `apply` turns a click or a keystroke back into an editor action.

use research_doc::{CellBody, Output};
use tpt_appfront_core::UITree;

use crate::editor::{CellKind, Editor};

/// What the window can ask the editor to do.
#[derive(Debug, Clone)]
pub enum Msg {
    Open(String),
    New,
    EditSource(usize, String),
    AddCode,
    AddMarkdown,
    Run,
    Save,
}

/// The whole window: documents on the left, the open document on the right.
pub fn view(editor: &Editor) -> UITree<Msg> {
    let documents = editor.documents().unwrap_or_default();
    UITree::container(|c| {
        c.heading(1, editor.project_name().to_string());
        c.container(|side| {
            side.heading(3, "Documents".to_string());
            for doc in &documents {
                side.button(doc.title.clone())
                    .on_click(Msg::Open(doc.id.to_string()));
            }
            side.button("New document").on_click(Msg::New);
        });
        c.container(|main| match editor.document() {
            None => {
                main.text("No document open.".to_string());
            }
            Some(doc) => {
                main.heading(2, doc.title.clone());
                main.button("Run").on_click(Msg::Run);
                main.button("Save").on_click(Msg::Save);
                main.button("Add code cell").on_click(Msg::AddCode);
                main.button("Add text cell").on_click(Msg::AddMarkdown);
                for (index, cell) in doc.cells.iter().enumerate() {
                    let (label, source, outputs) = match &cell.body {
                        CellBody::Code {
                            source, outputs, ..
                        } => ("Code", source.clone(), outputs.clone()),
                        CellBody::Markdown { source } => ("Text", source.clone(), Vec::new()),
                    };
                    main.heading(3, format!("{label} · {}", cell.id));
                    main.textarea(source)
                        .on_input(move |text| Msg::EditSource(index, text));
                    for output in &outputs {
                        main.text(output_line(output));
                    }
                }
            }
        });
        c.text(editor.status().to_string());
    })
}

/// Apply one message to the editor. A failure becomes the status line.
pub fn apply(editor: &mut Editor, msg: Msg) {
    let result = match msg {
        Msg::Open(id) => editor.open(&id),
        Msg::New => editor.create_untitled(),
        Msg::EditSource(index, text) => editor.set_source(index, &text),
        Msg::AddCode => editor.add_cell(CellKind::Code).map(|_| ()),
        Msg::AddMarkdown => editor.add_cell(CellKind::Markdown).map(|_| ()),
        Msg::Run => editor.run().map(|_| ()),
        Msg::Save => editor.save(),
    };
    if let Err(error) = result {
        editor.set_status(error.to_string());
    }
}

fn output_line(output: &Output) -> String {
    match output {
        Output::Text { text } => text.clone(),
        Output::Error { name, message } => format!("{name}: {message}"),
        Output::Table { columns, rows } => {
            let mut lines = vec![columns.join("\t")];
            lines.extend(rows.iter().map(|row| row.join("\t")));
            lines.join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;

    fn editor() -> (tempfile::TempDir, Editor) {
        let work = tempfile::tempdir().unwrap();
        let ws = Workspace::create(&work.path().join("p"), "p", "Project", 1).unwrap();
        (work, Editor::new(ws))
    }

    #[test]
    fn a_click_sequence_creates_edits_and_runs_a_document() {
        let (_work, mut ed) = editor();
        apply(&mut ed, Msg::New);
        assert_eq!(ed.document().unwrap().title, "Untitled 1");
        apply(&mut ed, Msg::AddCode);
        apply(&mut ed, Msg::EditSource(0, "3 * 3".into()));
        apply(&mut ed, Msg::Run);
        assert!(ed.status().contains("Ran 1 cell"), "{}", ed.status());
        apply(&mut ed, Msg::Save);
        assert!(ed.status().starts_with("Saved"));
    }

    #[test]
    fn failures_become_the_status_line_not_panics() {
        let (_work, mut ed) = editor();
        apply(&mut ed, Msg::Run);
        assert_eq!(ed.status(), "open a document first");
        apply(&mut ed, Msg::Open("missing".into()));
        assert_eq!(ed.status(), "no document called missing");
        apply(&mut ed, Msg::EditSource(4, "x".into()));
        assert!(ed.status().contains("no document") || ed.status().contains("open a document"));
    }

    #[test]
    fn the_view_builds_for_an_empty_and_an_open_project() {
        let (_work, mut ed) = editor();
        let _empty = view(&ed);
        apply(&mut ed, Msg::New);
        apply(&mut ed, Msg::AddMarkdown);
        let _open = view(&ed);
    }
}
