use std::path::PathBuf;

use super::acp::{AcpDriver, AcpRuntime};
use super::{command_exists, run_command, SpawnContext};
use crate::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus};
use crate::store::agents::AgentRuntime;

pub struct OpencodeAcpRuntime;

fn parse_opencode_models(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

impl AcpRuntime for OpencodeAcpRuntime {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Opencode
    }

    fn binary_name(&self) -> &str {
        "opencode"
    }

    fn build_acp_args(&self, _ctx: &SpawnContext) -> Vec<String> {
        // `opencode acp` has no --model or --variant flags; configuration is via opencode.json.
        vec!["acp".to_string()]
    }

    fn session_new_params(&self, ctx: &SpawnContext) -> serde_json::Value {
        // OpenCode requires `cwd` and `mcpServers` (array). MCP bridge is registered via
        // opencode.json (type "local"), so mcpServers here is empty.
        serde_json::json!({ "cwd": ctx.working_directory, "mcpServers": [] })
    }

    fn requires_session_id_in_prompt(&self) -> bool {
        true
    }

    fn write_mcp_config(&self, ctx: &SpawnContext) -> anyhow::Result<Option<PathBuf>> {
        // Build model ID: append reasoning variant as suffix if set (e.g. "openai/gpt-4o/low").
        let model_id = match &ctx.config.reasoning_effort {
            Some(variant) if !variant.is_empty() => {
                format!("{}/{}", ctx.config.model, variant)
            }
            _ => ctx.config.model.clone(),
        };

        // McpLocalConfig requires `command` as a single array (binary + args).
        // There is no separate `args` field; additionalProperties is false.
        let mcp_config = serde_json::json!({
            "model": model_id,
            "mcp": {
                "chat": {
                    "type": "local",
                    "command": [&ctx.bridge_binary, "bridge", "--agent-id", &ctx.agent_id, "--server-url", &ctx.server_url]
                }
            }
        });
        // OpenCode reads opencode.json from the working directory automatically.
        let config_path = std::path::Path::new(&ctx.working_directory).join("opencode.json");
        std::fs::write(&config_path, serde_json::to_string_pretty(&mcp_config)?)?;
        Ok(Some(config_path))
    }

    fn env_overrides(&self, _ctx: &SpawnContext) -> Vec<(String, Option<String>)> {
        vec![
            ("NO_COLOR".to_string(), Some("1".to_string())),
        ]
    }

    fn tool_prefix(&self) -> &str {
        "chat_"
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        if !command_exists("opencode") {
            return Ok(RuntimeStatus {
                runtime: "opencode".to_string(),
                installed: false,
                auth_status: None,
            });
        }

        let auth_status = run_command("opencode", &["--version"])
            .ok()
            .map(|result| {
                if result.success {
                    RuntimeAuthStatus::Authed
                } else {
                    RuntimeAuthStatus::Unauthed
                }
            })
            .unwrap_or(RuntimeAuthStatus::Unauthed);

        Ok(RuntimeStatus {
            runtime: "opencode".to_string(),
            installed: true,
            auth_status: Some(auth_status),
        })
    }

    fn list_models(&self) -> anyhow::Result<Vec<String>> {
        if !command_exists("opencode") {
            return Ok(Vec::new());
        }

        let result = run_command("opencode", &["models"])?;
        if !result.success {
            anyhow::bail!("failed to list opencode models: {}", result.stderr.trim());
        }

        Ok(parse_opencode_models(&result.stdout))
    }
}

/// OpenCode driver backed by the Agent Client Protocol.
pub type OpencodeDriver = AcpDriver<OpencodeAcpRuntime>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_opencode_models_skips_blank_lines() {
        assert_eq!(
            parse_opencode_models("\nopenai/gpt-5.4\n\nopenai/gpt-5.4-mini\n"),
            vec![
                "openai/gpt-5.4".to_string(),
                "openai/gpt-5.4-mini".to_string()
            ]
        );
    }
}
