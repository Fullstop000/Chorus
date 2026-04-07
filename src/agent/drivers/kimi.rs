use std::path::{Path, PathBuf};

use super::acp::{AcpDriver, AcpRuntime};
use super::{command_exists, home_dir, read_file, SpawnContext};
use crate::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus};
use crate::store::agents::AgentRuntime;

pub struct KimiAcpRuntime;

impl AcpRuntime for KimiAcpRuntime {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Kimi
    }

    fn binary_name(&self) -> &str {
        "kimi"
    }

    fn build_acp_args(&self, ctx: &SpawnContext) -> Vec<String> {
        let mcp_config_path =
            std::path::Path::new(&ctx.working_directory).join(".chorus-kimi-mcp.json");

        // Kimi supports native ACP via the `acp` subcommand.
        let mut args = vec![
            "acp".to_string(),
            "--work-dir".to_string(),
            ctx.working_directory.clone(),
            "--mcp-config-file".to_string(),
            mcp_config_path.to_string_lossy().into_owned(),
        ];

        if !ctx.config.model.is_empty() {
            args.push("--model".to_string());
            args.push(ctx.config.model.clone());
        }

        args
    }

    fn write_mcp_config(&self, ctx: &SpawnContext) -> anyhow::Result<Option<PathBuf>> {
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "chat": {
                    "command": ctx.bridge_binary,
                    "args": ["bridge", "--agent-id", &ctx.agent_id, "--server-url", &ctx.server_url]
                }
            }
        });
        let mcp_config_path =
            std::path::Path::new(&ctx.working_directory).join(".chorus-kimi-mcp.json");
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)?;
        Ok(Some(mcp_config_path))
    }

    fn env_overrides(&self, _ctx: &SpawnContext) -> Vec<(String, Option<String>)> {
        vec![
            ("NO_COLOR".to_string(), Some("1".to_string())),
        ]
    }

    fn tool_prefix(&self) -> &str {
        ""
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        detect_kimi_runtime_status("kimi", &home_dir())
    }

    fn list_models(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec!["kimi-code/kimi-for-coding".to_string()])
    }
}

fn detect_kimi_runtime_status(runtime_id: &str, home: &Path) -> anyhow::Result<RuntimeStatus> {
    if !command_exists("kimi") {
        return Ok(RuntimeStatus {
            runtime: runtime_id.to_string(),
            installed: false,
            auth_status: None,
        });
    }

    let auth_status = read_file(&home.join(".kimi/credentials/kimi-code.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .map(|payload| {
            let has_access = payload["access_token"]
                .as_str()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
            let has_refresh = payload["refresh_token"]
                .as_str()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
            if has_access || has_refresh {
                RuntimeAuthStatus::Authed
            } else {
                RuntimeAuthStatus::Unauthed
            }
        })
        .unwrap_or(RuntimeAuthStatus::Unauthed);

    Ok(RuntimeStatus {
        runtime: runtime_id.to_string(),
        installed: true,
        auth_status: Some(auth_status),
    })
}

/// Kimi driver backed by the Agent Client Protocol.
pub type KimiDriver = AcpDriver<KimiAcpRuntime>;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn kimi_runtime_status_reads_local_credentials() {
        let dir = tempdir().unwrap();
        let credentials_dir = dir.path().join(".kimi/credentials");
        std::fs::create_dir_all(&credentials_dir).unwrap();
        std::fs::write(
            credentials_dir.join("kimi-code.json"),
            r#"{"access_token":"token","refresh_token":""}"#,
        )
        .unwrap();

        let status = detect_kimi_runtime_status("kimi", dir.path()).unwrap();

        assert_eq!(status.runtime, "kimi");
        if status.installed {
            assert_eq!(status.auth_status, Some(RuntimeAuthStatus::Authed));
        } else {
            assert_eq!(status.auth_status, None);
        }
    }
}
