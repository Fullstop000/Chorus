pub mod claude;
pub mod codex;
pub mod prompt;

use std::process::Child;

/// Events parsed from agent CLI stdout.
#[derive(Debug, Clone)]
pub enum ParsedEvent {
    SessionInit { session_id: String },
    Thinking { text: String },
    Text { text: String },
    ToolCall { name: String, input: serde_json::Value },
    TurnEnd { session_id: Option<String> },
    Error { message: String },
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
    fn id(&self) -> &str;
    fn supports_stdin_notification(&self) -> bool;
    fn mcp_tool_prefix(&self) -> &str;
    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child>;
    fn parse_line(&self, line: &str) -> Vec<ParsedEvent>;
    fn encode_stdin_message(&self, text: &str, session_id: &str) -> Option<String>;
    fn build_system_prompt(&self, config: &crate::models::AgentConfig, agent_id: &str) -> String;
    fn tool_display_name(&self, name: &str) -> String;
    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String;
}
