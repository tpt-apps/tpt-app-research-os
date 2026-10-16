//! Computational documents: the cell model, its JSON form and `.ipynb`
//! import/export. The format is specified in `docs/DOCUMENT_FORMAT.md`.

mod document;
mod error;
mod ipynb;

pub use document::{Cell, CellBody, Document, Output, FORMAT_VERSION};
pub use error::{DocError, Result};
pub use ipynb::{export_ipynb, import_ipynb, ImportReport, HOSTED_LANGUAGES};

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-doc";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-doc");
    }
}
