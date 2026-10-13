//! Unified error type for the research crates.

/// Result alias using [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced by the shared research types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An identifier failed validation.
    #[error("invalid id: {0}")]
    InvalidId(String),

    /// A manifest or other document could not be parsed or written.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A manifest was written by a format version this build cannot read.
    #[error("unsupported format version {found} (this build supports up to {supported})")]
    UnsupportedFormatVersion { found: u32, supported: u32 },

    /// A manifest field has a value that is structurally valid but not allowed.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
}
