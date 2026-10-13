//! Shared types: project, run, claim ids, errors.

mod error;
mod id;
mod manifest;

pub use error::{Error, Result};
pub use id::{CellId, DocId, Id, ProjectId, ResultId, RunId};
pub use manifest::{ProjectManifest, FORMAT_VERSION};

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-core";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-core");
    }
}
