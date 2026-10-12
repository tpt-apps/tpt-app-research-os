//! Execution engine for document cells over tpt-math / tpt-science / tpt-mathr.

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-compute";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-compute");
    }
}
