//! Application model and editor for TPT Research OS. The window in `main.rs` is a
//! thin layer over this, so the behaviour can be tested without a display.

pub mod compare;
pub mod editor;
pub mod view;
pub mod workspace;

pub use compare::compare_lines;
pub use editor::{CellKind, Editor, EditorError};
pub use view::{apply, view, Msg};
pub use workspace::{Workspace, WorkspaceError};
