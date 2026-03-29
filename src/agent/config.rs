use serde::{Deserialize, Serialize};

use crate::store::agents::AgentEnvVar;
use crate::store::teams::TeamMembership;

/// Snapshot passed to the bridge when spawning an agent (includes team context).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent handle.
    pub name: String,
    /// Display name for prompts.
    pub display_name: String,
    /// Optional description for system prompt.
    pub description: Option<String>,
    /// Driver key.
    pub runtime: String,
    /// Model id.
    pub model: String,
    /// Active session id if resuming.
    pub session_id: Option<String>,
    /// Reasoning effort for Codex.
    pub reasoning_effort: Option<String>,
    /// Environment variables for the child process.
    pub env_vars: Vec<AgentEnvVar>,
    /// Team memberships injected into the agent's system prompt at spawn time.
    pub teams: Vec<TeamMembership>,
}
