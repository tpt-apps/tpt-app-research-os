//! Errors for the compute layer.

/// Result alias using [`ComputeError`].
pub type Result<T> = std::result::Result<T, ComputeError>;

/// Errors from running documents. A failing cell is not one of these: its error is
/// stored in the cell's outputs and the run carries on.
#[derive(Debug, thiserror::Error)]
pub enum ComputeError {
    /// A document could not be read or written.
    #[error(transparent)]
    Doc(#[from] research_doc::DocError),

    /// A shared type (id) was invalid.
    #[error(transparent)]
    Core(#[from] research_core::Error),

    /// The project store refused a write.
    #[error(transparent)]
    Store(#[from] research_store::StoreError),

    /// A limit was set to a value that cannot work.
    #[error("invalid limit: {0}")]
    InvalidLimit(String),
}
