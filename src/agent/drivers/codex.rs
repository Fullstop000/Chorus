use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::acp::{AcpDriver, AcpRuntime};
use super::{command_exists, run_command, SpawnContext};
use crate::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus};
use crate::agent::AgentRuntime;

pub struct CodexAcpRuntime;

impl AcpRuntime for CodexAcpRuntime {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Codex
    }

    fn binary_name(&self) -> &str {
        "codex-acp"
    }

    fn build_acp_args(&self, ctx: &SpawnContext) -> Vec<String> {
        let mut args = vec![
            // Run fully non-interactive: never prompt for approval, full sandbox access.
            "-c".to_string(),
            r#"approval_policy="never""#.to_string(),
            "-c".to_string(),
            r#"sandbox_mode="danger-full-access""#.to_string(),
        ];

        if let Some(reasoning_effort) = ctx.config.reasoning_effort.as_deref() {
            if let Ok(val) = serde_json::to_string(reasoning_effort) {
                args.push("-c".to_string());
                args.push(format!("model_reasoning_effort={val}"));
            }
        }

        if !ctx.config.model.is_empty() {
            if let Ok(val) = serde_json::to_string(&ctx.config.model) {
                args.push("-c".to_string());
                args.push(format!("model={val}"));
            }
        }

        args
    }

    // MCP bridge is registered via session/new mcpServers (ACP standard), not config file.
    fn write_mcp_config(&self, _ctx: &SpawnContext) -> anyhow::Result<Option<PathBuf>> {
        Ok(None)
    }

    fn session_new_params(&self, ctx: &SpawnContext) -> serde_json::Value {
        serde_json::json!({
            "cwd": ctx.working_directory,
            "mcpServers": [{
                "name": "chat",
                "command": ctx.bridge_binary,
                "args": ["bridge", "--agent-id", ctx.agent_id, "--server-url", ctx.server_url],
                "env": []
            }]
        })
    }

    fn requires_session_id_in_prompt(&self) -> bool {
        // codex-acp requires the sessionId from session/new to be included in session/prompt.
        true
    }

    fn env_overrides(&self, _ctx: &SpawnContext) -> Vec<(String, Option<String>)> {
        vec![
            ("NO_COLOR".to_string(), Some("1".to_string())),
        ]
    }

    fn pre_spawn_setup(&self, ctx: &SpawnContext) -> anyhow::Result<()> {
        // Codex requires a git repo in the working directory.
        let git_dir = std::path::Path::new(&ctx.working_directory).join(".git");
        if !git_dir.exists() {
            Command::new("git")
                .args(["init"])
                .current_dir(&ctx.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;

            let git_env = [
                ("GIT_AUTHOR_NAME", "slock"),
                ("GIT_AUTHOR_EMAIL", "slock@local"),
                ("GIT_COMMITTER_NAME", "slock"),
                ("GIT_COMMITTER_EMAIL", "slock@local"),
            ];

            Command::new("git")
                .args(["add", "-A"])
                .current_dir(&ctx.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;

            Command::new("git")
                .args(["commit", "--allow-empty", "-m", "init"])
                .current_dir(&ctx.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .envs(git_env)
                .status()?;
        }
        Ok(())
    }

    fn tool_prefix(&self) -> &str {
        "mcp_chat_"
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        if !command_exists("codex") {
            return Ok(RuntimeStatus {
                runtime: "codex".to_string(),
                installed: false,
                auth_status: None,
            });
        }

        let auth_status = run_command("codex", &["login", "status"])
            .ok()
            .map(|result| {
                let combined = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
                if result.success && combined.contains("logged in") {
                    RuntimeAuthStatus::Authed
                } else {
                    RuntimeAuthStatus::Unauthed
                }
            })
            .unwrap_or(RuntimeAuthStatus::Unauthed);

        Ok(RuntimeStatus {
            runtime: "codex".to_string(),
            installed: true,
            auth_status: Some(auth_status),
        })
    }

    fn list_models(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec![
            "gpt-5.4".to_string(),
            "gpt-5.4-mini".to_string(),
            "gpt-5.3-codex".to_string(),
            "gpt-5.2-codex".to_string(),
            "gpt-5.2".to_string(),
            "gpt-5.1-codex-max".to_string(),
            "gpt-5.1-codex-mini".to_string(),
        ])
    }
}

/// Codex driver backed by the Agent Client Protocol.
pub type CodexDriver = AcpDriver<CodexAcpRuntime>;
