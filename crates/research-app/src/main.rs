//! TPT Research OS desktop shell. This is the first spike: a native window built
//! with AppFront's canvas backend. It shows that the window opens and that a
//! button event reaches Rust. The document editor is not built yet.

use tpt_appfront_core::{Signal, UITree};

#[derive(Debug, Clone)]
enum Msg {
    Refresh,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let presses = Signal::new(0u32);

    let shown = presses.clone();
    let build_ui = move || -> UITree<Msg> {
        UITree::container(|c| {
            c.heading(1, "TPT Research OS");
            c.text(format!("Shell ready. Button presses: {}", shown.get()));
            c.button("Refresh").on_click(Msg::Refresh);
        })
    };

    let dispatch = move |msg: Msg| match msg {
        Msg::Refresh => presses.set(presses.get() + 1),
    };

    tpt_appfront_canvas::run_native("TPT Research OS", build_ui, dispatch)?;
    Ok(())
}
