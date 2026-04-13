pub mod v2;

pub mod acp;
pub mod claude;
pub mod claude_raw;
pub mod codex;
pub mod codex_raw;
pub mod kimi;
pub mod kimi_raw;
pub mod opencode;
pub mod opencode_raw;
pub mod prompt;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::process::Command;
use std::sync::{Arc, OnceLock};

use anyhow::Context;

use crate::agent::config::AgentConfig;
use crate::agent::runtime_status::RuntimeStatus;
use crate::agent::AgentRuntime;

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
    /// Deferred tool-call input received via `tool_call_update` with `rawInput`.
    /// ACP runtimes often send the initial `tool_call` with empty args, then
    /// deliver the real input in a subsequent `tool_call_update`.
    ToolCallUpdate {
        input: serde_json::Value,
    },
    ToolResult {
        content: String,
    },
    TurnEnd {
        session_id: Option<String>,
    },
    Error {
        message: String,
    },
    /// Pre-encoded bytes to write directly to the agent's stdin.
    /// Used by runtimes that need to write additional startup data after
    /// receiving the session/new response (e.g. kimi's deferred session/prompt).
    WriteStdin {
        data: String,
    },
    /// Emitted alongside WriteStdin when a `session/request_permission` arrives.
    /// The manager sets a flag so TurnEnd can send a follow-up session/prompt
    /// that lets kimi read the buffered approval and re-attempt the tool.
    PermissionRequested {
        /// The bare tool name from the `toolCall.title` field, if extractable.
        tool_name: Option<String>,
    },
}

/// Spawn context passed to drivers.
pub struct SpawnContext {
    pub agent_id: String,
    pub agent_name: String,
    pub config: AgentConfig,
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
    fn build_system_prompt(&self, config: &AgentConfig, agent_id: &str) -> String;
    /// Convert a raw tool name into the short label shown in the activity log.
    fn tool_display_name(&self, name: &str) -> String;
    /// Produce a compact summary of tool input for UI activity rows and tracing.
    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String;
    /// Detect whether the runtime is installed and authenticated on this machine.
    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus>;
    /// Return the runtime's currently supported model ids.
    fn list_models(&self) -> anyhow::Result<Vec<String>>;
    /// Whether the driver needs a pre-generated session ID before spawning.
    /// True for raw Kimi driver (stdin notifications require a session ID up front).
    /// False for ACP drivers (they negotiate sessions via session/new).
    fn needs_pregenerated_session_id(&self) -> bool {
        false
    }
}

/// ACP adapter binary names for each runtime.
/// When the adapter is installed, the ACP driver is preferred over the raw driver.
fn acp_adapter_binary(runtime: AgentRuntime) -> &'static str {
    runtime.acp_adaptor_binary()
}

/// Build a driver for the given runtime, preferring ACP when the adapter is available.
pub fn driver_for_runtime(runtime: AgentRuntime) -> Arc<dyn Driver> {
    let acp_binary = acp_adapter_binary(runtime);
    if command_exists(acp_binary) {
        match runtime {
            AgentRuntime::Claude => Arc::new(acp::AcpDriver::new(claude::ClaudeAcpRuntime)),
            AgentRuntime::Codex => Arc::new(acp::AcpDriver::new(codex::CodexAcpRuntime)),
            AgentRuntime::Kimi => Arc::new(acp::AcpDriver::new(kimi::KimiAcpRuntime)),
            AgentRuntime::Opencode => Arc::new(acp::AcpDriver::new(opencode::OpencodeAcpRuntime)),
        }
    } else {
        // Fallback to raw (1.0) driver when ACP adapter is not installed.
        match runtime {
            AgentRuntime::Claude => Arc::new(claude_raw::ClaudeRawDriver),
            AgentRuntime::Codex => Arc::new(codex_raw::CodexRawDriver),
            AgentRuntime::Kimi => Arc::new(kimi_raw::KimiRawDriver),
            AgentRuntime::Opencode => Arc::new(opencode_raw::OpencodeRawDriver),
        }
    }
}

/// Return all runtime drivers for status detection and model listing.
/// Uses raw drivers since these operations don't need ACP.
pub fn all_runtime_drivers() -> Vec<Arc<dyn Driver>> {
    vec![
        Arc::new(claude_raw::ClaudeRawDriver),
        Arc::new(codex_raw::CodexRawDriver),
        Arc::new(kimi_raw::KimiRawDriver),
        Arc::new(opencode_raw::OpencodeRawDriver),
    ]
}

pub(crate) fn command_exists(command: &str) -> bool {
    is_executable_in_dirs(command, &process_path_dirs())
        || is_executable_in_dirs(command, user_shell_path_dirs())
}

fn is_executable_in_dirs(command: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().map(|dir| dir.join(command)).any(|candidate| {
        fs::metadata(&candidate)
            .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
            .unwrap_or(false)
    })
}

fn process_path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

/// Resolve the user's interactive shell PATH once and cache it.
/// Handles any node/tool version manager (nvm, volta, fnm, etc.) that
/// hooks into the shell init files rather than the system PATH.
static USER_SHELL_PATH_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();

fn user_shell_path_dirs() -> &'static [PathBuf] {
    USER_SHELL_PATH_DIRS.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Command::new(&shell)
            .args(["-i", "-c", "echo $PATH"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| std::env::split_paths(s.trim()).collect())
            .unwrap_or_default()
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
