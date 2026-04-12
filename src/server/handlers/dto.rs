//! HTTP / UI response shapes assembled in the handler layer from store records.

use serde::{Deserialize, Serialize};

use crate::agent::runtime_status::RuntimeAuthStatus;
use crate::store::agents::Agent;
use crate::store::channels::Channel;
use crate::store::humans::Human;

/// Full agent-scoped workspace snapshot for bridge/CLI discovery.
///
/// The type name is historical because the wire contract is still exposed at
/// `/internal/agent/{agent_id}/server`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// User/team channels the subject has joined (excludes system rooms in this list).
    pub channels: Vec<ChannelInfo>,
    /// Built-in system channels (e.g. `#all`).
    pub system_channels: Vec<ChannelInfo>,
    /// All agent records with persisted status (activity may be merged in handlers).
    pub agents: Vec<AgentInfo>,
    /// Registered human users.
    pub humans: Vec<HumanInfo>,
}

/// Minimal payload for the UI shell before full server-info polling (sidebar bootstrap).
#[derive(Debug, Serialize, Deserialize)]
pub struct UiShellInfo {
    /// System channels shown in the sidebar; membership forced to joined for display.
    pub system_channels: Vec<ChannelInfo>,
    /// Humans listed in the sidebar.
    pub humans: Vec<HumanInfo>,
}

/// Channel row as returned to the UI / JSON APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    /// Stable channel id (omitted when serde skips none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Channel slug without leading `#`.
    pub name: String,
    /// Optional human-facing description.
    pub description: Option<String>,
    /// Whether the current viewer (`for_member` query context) is a member.
    pub joined: bool,
    /// API string for kind: `channel`, `dm`, `system`, `team`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_type: Option<String>,
    /// When true, normal users/agents cannot post.
    #[serde(default)]
    pub read_only: bool,
}

/// Agent summary for lists and agent detail header (before env vars).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Unique agent login name.
    pub name: String,
    /// Persisted lifecycle: `active`, `sleeping`, or `inactive`.
    pub status: String,
    /// Shown label in the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Optional longer blurb.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Full system prompt (rich persona prompts from templates).
    #[serde(rename = "systemPrompt", skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Driver key, e.g. `claude`, `codex`, `kimi`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// Model id passed to the driver.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Codex-only reasoning effort override.
    #[serde(rename = "reasoningEffort", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Opaque session id when the agent process is connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Short activity label from the runtime (`AgentManager` / activity map).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// Longer activity text for tooltips / panels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_detail: Option<String>,
}

/// Local machine status for a supported CLI runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeStatusInfo {
    /// Driver key, e.g. `claude`, `codex`, `kimi`.
    pub runtime: String,
    /// Whether the executable is present on the local machine.
    pub installed: bool,
    /// Authentication state when the runtime is installed.
    #[serde(rename = "authStatus", skip_serializing_if = "Option::is_none")]
    pub auth_status: Option<RuntimeAuthStatus>,
    /// Active driver mode: `"acp"` when the ACP adapter binary is installed, otherwise `"raw"`.
    #[serde(rename = "driverMode")]
    pub driver_mode: String,
}

/// Human user row for agent workspace snapshots and the UI shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanInfo {
    /// OS / login username used as human id.
    pub name: String,
}

impl From<(&Channel, bool)> for ChannelInfo {
    /// `bool` is whether the current viewer is a member (`joined`).
    fn from((channel, joined): (&Channel, bool)) -> Self {
        Self {
            id: Some(channel.id.clone()),
            name: channel.name.clone(),
            description: channel.description.clone(),
            joined,
            channel_type: Some(channel.channel_type.as_api_str().to_string()),
            read_only: false,
        }
    }
}

impl From<&Agent> for AgentInfo {
    /// Base agent row for list/detail; activity fields are filled by handlers when needed.
    fn from(agent: &Agent) -> Self {
        Self {
            name: agent.name.clone(),
            status: agent.status.as_str().to_string(),
            display_name: Some(agent.display_name.clone()),
            description: agent.description.clone(),
            system_prompt: agent.system_prompt.clone(),
            runtime: Some(agent.runtime.clone()),
            model: Some(agent.model.clone()),
            reasoning_effort: agent.reasoning_effort.clone(),
            session_id: agent.session_id.clone(),
            activity: None,
            activity_detail: None,
        }
    }
}

impl From<Human> for HumanInfo {
    fn from(human: Human) -> Self {
        Self { name: human.name }
    }
}
