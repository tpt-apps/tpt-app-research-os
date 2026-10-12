//! Keystone-backed project database and experiment tracking.

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-store";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-store");
    }
}
