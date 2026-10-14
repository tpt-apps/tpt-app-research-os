//! Optional, capability-restricted local agent helpers. An agent acts only through
//! capabilities it was explicitly given, built on `tpt-primitives`.

mod agent;
pub mod reproducibility;

pub use agent::{ActionRecord, Agent, AgentError, Denial};
pub use reproducibility::{check_document, check_with_agent, CellCheck, CheckError, ReproReport};

/// Crate name, used by smoke tests.
pub const NAME: &str = "research-agents";

#[cfg(test)]
mod tests {
    #[test]
    fn name() {
        assert_eq!(super::NAME, "research-agents");
    }
}
