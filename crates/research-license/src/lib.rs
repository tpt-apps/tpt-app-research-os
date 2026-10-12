//! Offline licence key verification for Gumroad builds.

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-license";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-license");
    }
}
