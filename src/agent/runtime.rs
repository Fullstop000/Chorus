use serde::{Deserialize, Serialize};

/// Supported local agent runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntime {
    Claude,
    Codex,
    Kimi,
    Opencode,
}

impl AgentRuntime {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Opencode => "opencode",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "kimi" => Some(Self::Kimi),
            "opencode" => Some(Self::Opencode),
            _ => None,
        }
    }

    pub const fn binary_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Opencode => "opencode",
        }
    }

    pub const fn acp_adaptor_binary(self) -> &'static str {
        match self {
            Self::Claude => "claude-agent-acp",
            Self::Codex => "codex-acp",
            // Kimi and OpenCode have native ACP support via subcommands,
            // so we check for the main binary itself.
            Self::Kimi => "kimi",
            Self::Opencode => "opencode",
        }
    }
}
