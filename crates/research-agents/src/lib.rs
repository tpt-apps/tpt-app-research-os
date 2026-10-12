//! Optional capability-restricted local agent helpers.

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-agents";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-agents");
    }
}
