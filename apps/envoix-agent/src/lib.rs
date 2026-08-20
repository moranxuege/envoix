//! Reusable host for the persistent desktop Envoix Agent.

#[cfg(any(unix, windows))]
#[path = "unix_agent.rs"]
mod agent;

#[cfg(any(unix, windows))]
pub use agent::{AgentHost, AgentHostConfiguration, AgentShutdownHandle, run_cli};
