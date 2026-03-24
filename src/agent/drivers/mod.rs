pub mod claude;
pub mod codex;
pub mod prompt;

use std::process::Child;

/// Events parsed from agent CLI stdout.
#[derive(Debug, Clone)]
pub enum ParsedEvent {
    SessionInit {
        session_id: String,
    },
    Thinking {
        text: String,
    },
    Text {
        text: String,
    },
    ToolCall {
        name: String,
        input: serde_json::Value,
    },
    TurnEnd {
        session_id: Option<String>,
    },
    Error {
        message: String,
    },
}

/// Spawn context passed to drivers.
pub struct SpawnContext {
    pub agent_id: String,
    pub agent_name: String,
    pub config: crate::models::AgentConfig,
    pub prompt: String,
    pub working_directory: String,
    pub bridge_binary: String,
    pub server_url: String,
}

/// Runtime driver for a specific CLI (Claude, Codex, etc.)
pub trait Driver: Send + Sync {
    /// Return the stable runtime identifier persisted in agent records.
    fn id(&self) -> &str;
    /// Report whether the runtime can be nudged via stdin after startup.
    fn supports_stdin_notification(&self) -> bool;
    /// Return the MCP tool namespace prefix emitted by this runtime.
    fn mcp_tool_prefix(&self) -> &str;
    /// Launch the runtime process with the prepared spawn context.
    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child>;
    /// Parse one stdout line into zero or more normalized runtime events.
    fn parse_line(&self, line: &str) -> Vec<ParsedEvent>;
    /// Encode a wake-up notification for stdin-based runtimes, if supported.
    fn encode_stdin_message(&self, text: &str, session_id: &str) -> Option<String>;
    /// Build the runtime-specific system prompt for a given agent configuration.
    fn build_system_prompt(&self, config: &crate::models::AgentConfig, agent_id: &str) -> String;
    /// Convert a raw tool name into the short label shown in the activity log.
    fn tool_display_name(&self, name: &str) -> String;
    /// Produce a compact summary of tool input for UI activity rows and tracing.
    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String;
}
