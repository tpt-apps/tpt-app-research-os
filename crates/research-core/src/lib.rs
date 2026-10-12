//! Shared types: project, run, claim ids, errors.

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-core";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-core");
    }
}
