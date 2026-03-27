pub mod claude;
pub mod codex;
pub mod kimi;
pub mod prompt;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::process::Command;
use std::sync::Arc;

use anyhow::Context;

use crate::agent::runtime_status::RuntimeStatus;
use crate::store::agents::AgentRuntime;

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
    pub config: crate::store::agents::AgentConfig,
    pub prompt: String,
    pub working_directory: String,
    pub bridge_binary: String,
    pub server_url: String,
}

/// Runtime driver for a specific CLI (Claude, Codex, etc.)
pub trait Driver: Send + Sync {
    /// Return the stable runtime identifier persisted in agent records.
    fn runtime(&self) -> AgentRuntime;
    fn id(&self) -> &str {
        self.runtime().as_str()
    }
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
    fn build_system_prompt(
        &self,
        config: &crate::store::agents::AgentConfig,
        agent_id: &str,
    ) -> String;
    /// Convert a raw tool name into the short label shown in the activity log.
    fn tool_display_name(&self, name: &str) -> String;
    /// Produce a compact summary of tool input for UI activity rows and tracing.
    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String;
    /// Detect whether the runtime is installed and authenticated on this machine.
    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus>;
}

pub fn all_runtime_drivers() -> Vec<Arc<dyn Driver>> {
    vec![
        Arc::new(claude::ClaudeDriver),
        Arc::new(codex::CodexDriver),
        Arc::new(kimi::KimiDriver),
    ]
}

pub(crate) fn command_exists(command: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|dir| dir.join(command))
        .any(|candidate| {
            fs::metadata(&candidate)
                .map(|metadata| metadata.is_file() && (metadata.permissions().mode() & 0o111) != 0)
                .unwrap_or(false)
        })
}

pub(crate) fn run_command(program: &str, args: &[&str]) -> anyhow::Result<CommandProbeResult> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program} {}", args.join(" ")))?;
    Ok(CommandProbeResult {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub(crate) fn read_file(path: &Path) -> anyhow::Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandProbeResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}
