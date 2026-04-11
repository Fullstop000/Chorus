use serde::{Deserialize, Serialize};

use crate::store::agents::AgentEnvVar;

/// Snapshot passed to the bridge when spawning an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent handle.
    pub name: String,
    /// Display name for prompts.
    pub display_name: String,
    /// Optional description for system prompt.
    pub description: Option<String>,
    /// Full system prompt (rich template prompts go here; takes precedence over description).
    pub system_prompt: Option<String>,
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
}
