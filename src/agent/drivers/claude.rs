use std::path::PathBuf;

use super::acp::{AcpDriver, AcpRuntime};
use super::{command_exists, run_command, SpawnContext};
use crate::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus};
use crate::agent::AgentRuntime;

pub struct ClaudeAcpRuntime;

impl AcpRuntime for ClaudeAcpRuntime {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Claude
    }

    fn binary_name(&self) -> &str {
        "claude-agent-acp"
    }

    fn build_acp_args(&self, _ctx: &SpawnContext) -> Vec<String> {
        // claude-agent-acp is a pure ACP stdio agent (Claude Agent SDK adapter).
        // No CLI flags — MCP servers and model are configured via session/new params.
        vec![]
    }

    fn session_new_params(&self, ctx: &SpawnContext) -> serde_json::Value {
        serde_json::json!({
            "cwd": ctx.working_directory,
            "mcpServers": [{
                "name": "chat",
                "command": ctx.bridge_binary,
                "args": ["bridge", "--agent-id", &ctx.agent_id, "--server-url", &ctx.server_url]
            }]
        })
    }

    fn requires_session_id_in_prompt(&self) -> bool {
        true
    }

    fn write_mcp_config(&self, _ctx: &SpawnContext) -> anyhow::Result<Option<PathBuf>> {
        // MCP servers are registered via session/new params, not a config file.
        Ok(None)
    }

    fn env_overrides(&self, _ctx: &SpawnContext) -> Vec<(String, Option<String>)> {
        vec![
            // Remove CLAUDECODE to avoid nested invocation detection
            ("CLAUDECODE".to_string(), None),
        ]
    }

    fn tool_prefix(&self) -> &str {
        "mcp__chat__"
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        if !command_exists("claude") {
            return Ok(RuntimeStatus {
                runtime: "claude".to_string(),
                installed: false,
                auth_status: None,
            });
        }

        let auth_status = run_command("claude", &["auth", "status"])
            .ok()
            .and_then(|result| {
                if !result.success {
                    return Some(RuntimeAuthStatus::Unauthed);
                }
                let payload: serde_json::Value = serde_json::from_str(&result.stdout).ok()?;
                Some(if payload["loggedIn"].as_bool().unwrap_or(false) {
                    RuntimeAuthStatus::Authed
                } else {
                    RuntimeAuthStatus::Unauthed
                })
            })
            .unwrap_or(RuntimeAuthStatus::Unauthed);

        Ok(RuntimeStatus {
            runtime: "claude".to_string(),
            installed: true,
            auth_status: Some(auth_status),
        })
    }

    fn list_models(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec![
            "sonnet".to_string(),
            "opus".to_string(),
            "haiku".to_string(),
        ])
    }
}

/// Claude driver backed by the Agent Client Protocol.
pub type ClaudeDriver = AcpDriver<ClaudeAcpRuntime>;
