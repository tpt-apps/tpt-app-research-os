//! Computational document format (cells, narrative, results) and .ipynb import/export.

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-doc";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-doc");
    }
}
