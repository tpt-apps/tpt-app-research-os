//! Project directory and Keystone-backed store: experiment tracking, run records,
//! results, computational documents and content-addressed blobs. Everything stays
//! under the project directory.

mod blobs;
mod documents;
mod error;
mod project;
mod tracking;

pub use blobs::MAX_BLOB_BYTES;
pub use documents::DocumentSummary;
pub use error::{Result, StoreError};
pub use project::{ProjectStore, SCHEMA_VERSION};
pub use tracking::{Run, StoredResult, Value};

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-store";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-store");
    }
}
