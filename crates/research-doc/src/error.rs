//! Errors for document parsing, validation and notebook conversion.

/// Result alias using [`DocError`].
pub type Result<T> = std::result::Result<T, DocError>;

/// Errors produced by the document format.
#[derive(Debug, thiserror::Error)]
pub enum DocError {
    /// A shared type (id) was invalid.
    #[error(transparent)]
    Core(#[from] research_core::Error),

    /// JSON could not be read or written.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// The document was written by a newer format version than this build reads.
    #[error("unsupported document format version {found} (this build supports up to {supported})")]
    UnsupportedFormatVersion { found: u32, supported: u32 },

    /// The document breaks a rule in `docs/DOCUMENT_FORMAT.md`.
    #[error("invalid document: {0}")]
    Invalid(String),

    /// A notebook is not one this crate can convert.
    #[error("notebook error: {0}")]
    Notebook(String),
}
