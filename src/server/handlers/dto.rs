//! HTTP / UI response shapes assembled in the handler layer from store records.

use serde::{Deserialize, Serialize};

use crate::store::agents::{Agent, Human};
use crate::store::channels::{Channel, ChannelType};
use crate::store::Store;

/// Full workspace snapshot for an agent bridge client (channels, system rooms, agents, humans).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// User/team channels the subject has joined (excludes system rooms in this list).
    pub channels: Vec<ChannelInfo>,
    /// Built-in system channels (e.g. `#all`, `#shared-memory`).
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
    /// When true, normal users/agents cannot post (e.g. `#shared-memory`).
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

/// Human user row for server info and shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanInfo {
    /// OS / login username used as human id.
    pub name: String,
}

impl From<(&Channel, bool)> for ChannelInfo {
    /// `bool` is whether the current viewer is a member (`joined`).
    fn from((channel, joined): (&Channel, bool)) -> Self {
        let read_only = matches!(channel.channel_type, ChannelType::System)
            && Store::is_system_channel_read_only(&channel.name);
        Self {
            id: Some(channel.id.clone()),
            name: channel.name.clone(),
            description: channel.description.clone(),
            joined,
            channel_type: Some(channel.channel_type.as_api_str().to_string()),
            read_only,
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
