//! Application model for TPT Research OS. The window in `main.rs` is a thin layer
//! over this, so the behaviour can be tested without a display.

pub mod workspace;

pub use workspace::{Workspace, WorkspaceError};
