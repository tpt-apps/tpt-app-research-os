//! Errors for the project store.

use std::path::PathBuf;

/// Result alias using [`StoreError`].
pub type Result<T> = std::result::Result<T, StoreError>;

/// Errors produced by the project store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Another handle in this process or another process has the project open.
    #[error("project at {0} is already open")]
    Locked(PathBuf),

    /// A file operation failed.
    #[error("storage I/O failed: {0}")]
    Io(#[from] std::io::Error),

    /// The Keystone database reported an error.
    #[error("database error: {0}")]
    Database(String),

    /// A document could not be serialized to JSON.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A shared type (id, manifest) was invalid.
    #[error(transparent)]
    Core(#[from] research_core::Error),

    /// `create` was called on a directory that already holds a project.
    #[error("a project already exists at {0}")]
    AlreadyExists(PathBuf),

    /// `open` was called on a directory with no `manifest.json`.
    #[error("no project found at {0}")]
    ProjectMissing(PathBuf),

    /// The store was written by a newer build than this one.
    #[error("store schema version {found} is newer than this build supports ({supported})")]
    SchemaTooNew { found: u32, supported: u32 },

    /// Stored data does not match what this build expects.
    #[error("stored data is corrupt: {0}")]
    Corrupt(String),

    /// A result refers to a blob that is not stored.
    #[error("no blob with hash {0}")]
    BlobNotFound(String),

    /// A document failed validation or could not be converted.
    #[error(transparent)]
    Document(#[from] research_doc::DocError),

    /// A run referenced by a result, or requested by id, does not exist.
    #[error("no run with id {0}")]
    RunNotFound(String),

    /// A run or result has a field that is not allowed.
    #[error("invalid {what}: {reason}")]
    Invalid { what: &'static str, reason: String },
}
