//! Self-contained project packages: zip + manifest + signature + optional encryption.

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-package";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-package");
    }
}
