//! A local agent that may act only through capabilities it has been explicitly
//! given. Built on `tpt-primitives`' capability model.
//!
//! Rules:
//! - An agent starts with no capabilities, so every action is denied.
//! - An agent does nothing until it is started, and it is stopped by default.
//! - An action is allowed only if some held capability names exactly that
//!   (resource, action) pair and passes `valid_authority` at the given time.
//! - Every attempt, allowed or denied, is written to the action log.
//!
//! Lease and epoch bindings are not supplied here, so a capability bound to one
//! is denied. Those bindings need a lease or epoch source the app does not have yet.

use tpt_primitives::capability::{valid_authority, AuthorityError, Capability, Grant, Principal};
use tpt_primitives::LogicalTime;

/// Why an action was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    /// The agent is not running.
    NotRunning,
    /// No held capability names this action.
    NoCapability,
    /// A capability names this action but is not valid authority now.
    NotAuthorized(AuthorityError),
}

/// Errors from agent operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// The agent's name is not a valid principal label.
    InvalidName(String),
    /// A capability was given to an agent that is not its holder.
    NotTheHolder,
    /// An action was refused.
    Denied(Denial),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(reason) => write!(f, "invalid agent name: {reason}"),
            Self::NotTheHolder => write!(f, "capability is held by a different principal"),
            Self::Denied(Denial::NotRunning) => write!(f, "agent is not running"),
            Self::Denied(Denial::NoCapability) => write!(f, "no capability allows this action"),
            Self::Denied(Denial::NotAuthorized(e)) => write!(f, "capability is not valid: {e:?}"),
        }
    }
}

impl std::error::Error for AgentError {}

/// One attempted action, as recorded in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRecord {
    pub at: LogicalTime,
    pub action: Grant,
    /// `None` if allowed, otherwise the reason it was refused.
    pub denial: Option<Denial>,
}

/// A capability-restricted agent.
#[derive(Debug)]
pub struct Agent {
    principal: Principal,
    capabilities: Vec<Capability>,
    running: bool,
    log: Vec<ActionRecord>,
}

impl Agent {
    /// A new, stopped agent with no capabilities.
    pub fn new(name: &str) -> Result<Self, AgentError> {
        let principal = Principal::new(name).map_err(|e| AgentError::InvalidName(e.to_string()))?;
        Ok(Self {
            principal,
            capabilities: Vec::new(),
            running: false,
            log: Vec::new(),
        })
    }

    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    /// Whether the agent is running. The UI should show a clear indicator while it is.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Start the agent. Capabilities still have to allow each action.
    pub fn start(&mut self) {
        self.running = true;
    }

    /// Stop the agent. Every later action is denied until it is started again.
    pub fn stop(&mut self) {
        self.running = false;
    }

    /// Give the agent a capability. Only a capability held by this agent's
    /// principal is accepted, so an agent cannot receive authority meant for another.
    pub fn grant(&mut self, capability: Capability) -> Result<(), AgentError> {
        if capability.grant.holder != self.principal {
            return Err(AgentError::NotTheHolder);
        }
        self.capabilities.push(capability);
        Ok(())
    }

    /// Revoke every held capability at `at`. Actions are denied afterwards.
    pub fn revoke_all(&mut self, at: LogicalTime) {
        for capability in &mut self.capabilities {
            capability.revoke(at);
        }
    }

    /// Ask to perform `action` at logical time `at`. Returns `Ok` only if the
    /// action is allowed. Either way the attempt is logged.
    pub fn perform(&mut self, action: Grant, at: LogicalTime) -> Result<(), AgentError> {
        let denial = self.decide(&action, at);
        self.log.push(ActionRecord {
            at,
            action,
            denial: denial.clone(),
        });
        match denial {
            None => Ok(()),
            Some(reason) => Err(AgentError::Denied(reason)),
        }
    }

    /// Every attempted action, in the order attempted.
    pub fn log(&self) -> &[ActionRecord] {
        &self.log
    }

    fn decide(&self, action: &Grant, at: LogicalTime) -> Option<Denial> {
        if !self.running {
            return Some(Denial::NotRunning);
        }
        let mut last_refusal = None;
        let mut named = false;
        for capability in &self.capabilities {
            if !capability.grant.grants.contains(action) {
                continue;
            }
            named = true;
            match valid_authority(capability, None, None, at) {
                Ok(()) => return None,
                Err(e) => last_refusal = Some(e),
            }
        }
        match (named, last_refusal) {
            (false, _) => Some(Denial::NoCapability),
            (true, Some(e)) => Some(Denial::NotAuthorized(e)),
            (true, None) => Some(Denial::NoCapability),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_primitives::capability::AuthorityDomain;

    fn domain() -> AuthorityDomain {
        AuthorityDomain::new("research").unwrap()
    }

    fn grant(resource: &str, action: &str) -> Grant {
        Grant::new(resource, action).unwrap()
    }

    fn capability_for(agent: &Agent, grants: Vec<Grant>) -> Capability {
        Capability::mint(domain(), agent.principal().clone(), grants, false).unwrap()
    }

    fn t(ticks: u64) -> LogicalTime {
        LogicalTime::new(ticks)
    }

    #[test]
    fn with_no_capabilities_every_action_is_denied() {
        let mut agent = Agent::new("assistant").unwrap();
        agent.start();
        let r = agent.perform(grant("run/1", "read"), t(1));
        assert_eq!(r, Err(AgentError::Denied(Denial::NoCapability)));
    }

    #[test]
    fn a_stopped_agent_is_denied_even_with_a_capability() {
        let mut agent = Agent::new("assistant").unwrap();
        let cap = capability_for(&agent, vec![grant("run/1", "read")]);
        agent.grant(cap).unwrap();
        let r = agent.perform(grant("run/1", "read"), t(1));
        assert_eq!(r, Err(AgentError::Denied(Denial::NotRunning)));
        assert!(!agent.is_running());
    }

    #[test]
    fn a_capability_allows_exactly_its_grants() {
        let mut agent = Agent::new("assistant").unwrap();
        let cap = capability_for(&agent, vec![grant("run/1", "read")]);
        agent.grant(cap).unwrap();
        agent.start();

        assert!(agent.perform(grant("run/1", "read"), t(1)).is_ok());
        assert_eq!(
            agent.perform(grant("run/1", "write"), t(2)),
            Err(AgentError::Denied(Denial::NoCapability))
        );
        assert_eq!(
            agent.perform(grant("run/2", "read"), t(3)),
            Err(AgentError::Denied(Denial::NoCapability))
        );
    }

    #[test]
    fn a_capability_held_by_someone_else_is_refused() {
        let mut agent = Agent::new("assistant").unwrap();
        let other = Agent::new("someone-else").unwrap();
        let cap = capability_for(&other, vec![grant("run/1", "read")]);
        assert_eq!(agent.grant(cap), Err(AgentError::NotTheHolder));
    }

    #[test]
    fn revoked_capabilities_stop_working() {
        let mut agent = Agent::new("assistant").unwrap();
        let cap = capability_for(&agent, vec![grant("run/1", "read")]);
        agent.grant(cap).unwrap();
        agent.start();
        assert!(agent.perform(grant("run/1", "read"), t(1)).is_ok());

        agent.revoke_all(t(2));
        let r = agent.perform(grant("run/1", "read"), t(3));
        assert!(matches!(
            r,
            Err(AgentError::Denied(Denial::NotAuthorized(_)))
        ));
    }

    #[test]
    fn attenuated_capabilities_lose_the_removed_grants() {
        let mut agent = Agent::new("assistant").unwrap();
        let full = capability_for(
            &agent,
            vec![grant("run/1", "read"), grant("run/1", "write")],
        );
        let reduced = full.attenuate(|g| g.action == "read").unwrap();
        agent.grant(reduced).unwrap();
        agent.start();

        assert!(agent.perform(grant("run/1", "read"), t(1)).is_ok());
        assert_eq!(
            agent.perform(grant("run/1", "write"), t(2)),
            Err(AgentError::Denied(Denial::NoCapability))
        );
    }

    #[test]
    fn every_attempt_is_logged_in_order_with_its_outcome() {
        let mut agent = Agent::new("assistant").unwrap();
        let cap = capability_for(&agent, vec![grant("run/1", "read")]);
        agent.grant(cap).unwrap();
        agent.start();

        let _ = agent.perform(grant("run/1", "read"), t(1));
        let _ = agent.perform(grant("run/1", "delete"), t(2));
        agent.stop();
        let _ = agent.perform(grant("run/1", "read"), t(3));

        let log = agent.log();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].denial, None);
        assert_eq!(log[1].denial, Some(Denial::NoCapability));
        assert_eq!(log[2].denial, Some(Denial::NotRunning));
        assert_eq!(log[1].at, t(2));
    }

    #[test]
    fn invalid_agent_name_is_rejected() {
        assert!(matches!(Agent::new(""), Err(AgentError::InvalidName(_))));
    }
}
