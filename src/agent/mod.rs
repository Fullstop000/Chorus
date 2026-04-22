pub mod activity_log;
pub mod config;
pub mod drivers;
mod event_forwarder;
pub mod lifecycle;
pub mod manager;
pub mod process_status;
pub mod runtime_catalog;
pub mod runtime_status;
pub mod templates;
pub mod trace;
pub mod workspace;

pub use lifecycle::AgentLifecycle;

use serde::{Deserialize, Serialize};

/// Supported local agent runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntime {
    Claude,
    Codex,
    Kimi,
    Opencode,
    Gemini,
}

impl AgentRuntime {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Opencode => "opencode",
            Self::Gemini => "gemini",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "kimi" => Some(Self::Kimi),
            "opencode" => Some(Self::Opencode),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }
}
