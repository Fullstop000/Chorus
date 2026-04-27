//! Kimi runtime driver.
//!
//! All shared ACP-native plumbing (reader loop, response routing, session
//! lifecycle, cancel/close, ensure_started) lives in the
//! [`super::acp_native`] module. This file holds only the kimi-specific
//! pieces:
//!
//! - MCP config construction: the on-disk `.chorus-kimi-mcp.json` consumed
//!   via `--mcp-config-file`, plus the JSON shape sent in `session/new`
//!   params.
//! - `spawn_kimi`: writes the MCP config file and spawns
//!   `kimi --work-dir <wd> --mcp-config-file <path> [--model <model>] acp`.
//! - probe + list_models.
//! - The standing system prompt prepended to the first user turn — kimi has
//!   no `--system-prompt` flag and `--agent-file` is silently ignored, so
//!   the only place to anchor system rules is the leading user-role text on
//!   turn 1.

use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;

use crate::agent::AgentRuntime;
use crate::utils::cmd::{command_exists, home_dir, read_file};

use super::acp_native::{
    self, AcpDriverConfig, AcpNativeCore, InitPromptStrategy, SpawnFut, SpawnedChild,
};
use super::*;

// ---------------------------------------------------------------------------
// MCP config construction
// ---------------------------------------------------------------------------

/// Build the `.chorus-kimi-mcp.json` config file contents.
///
/// Produces the remote HTTP MCP shape, connecting the runtime to the shared
/// bridge at `{endpoint}/mcp`. Kimi requires `transport: "http"` alongside
/// `url` — without it, the runtime defaults to stdio and fails to connect.
/// Verified against the format emitted by `kimi mcp add --transport http`.
fn build_mcp_config_file(bridge_endpoint: &str, agent_key: &str) -> serde_json::Value {
    let url = super::bridge_mcp_url(bridge_endpoint);
    serde_json::json!({
        "mcpServers": {
            "chat": {
                "url": url,
                "transport": "http",
                "headers": {
                    "X-Agent-Id": agent_key
                }
            }
        }
    })
}

/// Build the `mcpServers` array for the ACP `session/new` inline params.
///
/// ACP spec for HTTP MCP servers in session/new params requires:
///   - `type: "http"` (NOT `transport: "http"` like Kimi's file config format)
///   - `headers` array (required, can be empty)
///
/// See <https://agentclientprotocol.com/protocol/session-setup> — sending
/// the wrong shape produces ACP "Invalid params" errors.
fn build_acp_mcp_servers(bridge_endpoint: &str, agent_key: &str) -> serde_json::Value {
    let url = super::bridge_mcp_url(bridge_endpoint);
    serde_json::json!([{
        "type": "http",
        "name": "chat",
        "url": url,
        "headers": [
            {"name": "X-Agent-Id", "value": agent_key}
        ]
    }])
}

// ---------------------------------------------------------------------------
// Standing system prompt for the first turn
// ---------------------------------------------------------------------------

/// Kimi-flavored system prompt prepended to the first user turn. Kimi's
/// `acp` subcommand silently ignores `--agent-file`, and `--wire` is
/// single-session-per-process (would break Chorus's multi-session
/// multiplexing). The least-bad place to teach Kimi the chat protocol is
/// the leading user-role text on turn 1.
fn build_kimi_standing_prompt(spec: &AgentSpec) -> String {
    super::prompt::build_system_prompt(
        spec,
        &super::prompt::PromptOptions {
            tool_prefix: String::new(),
            extra_critical_rules: vec![
                "- Do NOT use shell commands to send or receive messages. The MCP tools handle everything.".into(),
            ],
            post_startup_notes: Vec::new(),
            include_stdin_notification_section: true,
            message_notification_style: super::prompt::MessageNotificationStyle::Direct,
        },
    )
}

// ---------------------------------------------------------------------------
// Spawn child
// ---------------------------------------------------------------------------

fn spawn_kimi(spec: Arc<AgentSpec>, key: AgentKey) -> SpawnFut {
    Box::pin(async move {
        let wd = &spec.working_directory;
        let mcp_config_path = wd.join(".chorus-kimi-mcp.json");
        let mcp_config = build_mcp_config_file(&spec.bridge_endpoint, &key);
        tokio::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)
            .await
            .context("failed to write MCP config")?;

        let mcp_path_str = mcp_config_path.to_string_lossy().into_owned();
        let wd_str = wd.to_string_lossy().into_owned();
        let mut args = vec![
            "--work-dir".to_string(),
            wd_str,
            "--mcp-config-file".to_string(),
            mcp_path_str,
        ];
        if !spec.model.is_empty() {
            args.push("--model".to_string());
            args.push(spec.model.clone());
        }
        args.push("acp".to_string());

        let mut cmd = Command::new("kimi");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("FORCE_COLOR", "0")
            .env("NO_COLOR", "1");
        for ev in &spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let child = cmd.spawn().context("failed to spawn kimi")?;
        Ok(SpawnedChild { child })
    })
}

// ---------------------------------------------------------------------------
// Per-driver static registry + config
// ---------------------------------------------------------------------------

static KIMI_REGISTRY: AgentRegistry<AcpNativeCore> = AgentRegistry::new();

static KIMI_CFG: AcpDriverConfig = AcpDriverConfig {
    name: "kimi",
    runtime: AgentRuntime::Kimi,
    init_prompt_strategy: InitPromptStrategy::Immediate,
    initialized_notification_payload: None,
    session_load_includes_mcp: true,
    emit_starting_lifecycle: true,
    build_session_new_mcp_servers: build_acp_mcp_servers,
    build_first_prompt_prefix: Some(build_kimi_standing_prompt),
    spawn_child: spawn_kimi,
    registry: &KIMI_REGISTRY,
};

// ---------------------------------------------------------------------------
// KimiDriver — thin RuntimeDriver wrapper.
// ---------------------------------------------------------------------------

pub struct KimiDriver;

#[async_trait]
impl RuntimeDriver for KimiDriver {
    fn runtime(&self) -> AgentRuntime {
        KIMI_CFG.runtime
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        if !command_exists("kimi") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::AcpNative,
                capabilities: CapabilitySet::MODEL_LIST,
            });
        }

        let home = home_dir();
        let auth = read_file(&home.join(".kimi/credentials/kimi-code.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .map(|payload| {
                let has_access = payload["access_token"]
                    .as_str()
                    .is_some_and(|v| !v.trim().is_empty());
                let has_refresh = payload["refresh_token"]
                    .as_str()
                    .is_some_and(|v| !v.trim().is_empty());
                if has_access || has_refresh {
                    ProbeAuth::Authed
                } else {
                    ProbeAuth::Unauthed
                }
            })
            .unwrap_or(ProbeAuth::Unauthed);

        Ok(RuntimeProbe {
            auth,
            transport: TransportKind::AcpNative,
            capabilities: CapabilitySet::MODEL_LIST,
        })
    }

    async fn login(&self) -> anyhow::Result<LoginOutcome> {
        Ok(LoginOutcome::Failed {
            reason: "kimi does not support login via Chorus".into(),
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo::from_id("kimi-code/kimi-for-coding".into())])
    }

    async fn open_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        intent: SessionIntent,
    ) -> anyhow::Result<SessionAttachment> {
        acp_native::open_session(&KIMI_CFG, key, spec, intent).await
    }
}

// ---------------------------------------------------------------------------
// Tests — kimi-specific only. Generic ACP-native plumbing tests live in
// `acp_native::tests`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test-kimi".to_string(),
            description: None,
            system_prompt: None,
            model: "kimi-code/kimi-for-coding".to_string(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_endpoint: "http://127.0.0.1:1".to_string(),
        }
    }

    #[tokio::test]
    async fn probe_not_installed() {
        let driver = KimiDriver;
        let probe = driver.probe().await.unwrap();
        if probe.auth == ProbeAuth::NotInstalled {
            assert_eq!(probe.transport, TransportKind::AcpNative);
            assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
        }
    }

    #[tokio::test]
    async fn list_models_returns_kimi_default() {
        let driver = KimiDriver;
        let models = driver.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "kimi-code/kimi-for-coding");
    }

    #[tokio::test]
    async fn open_session_returns_idle() {
        let driver = KimiDriver;
        let key = format!("kimi-agent-idle-{}", uuid::Uuid::new_v4());
        let result = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        assert!(matches!(result.session.process_state(), ProcessState::Idle));
        KIMI_REGISTRY.remove(&key);
    }

    #[test]
    fn build_mcp_config_file_http_shape() {
        let config = build_mcp_config_file("http://127.0.0.1:4321", "tok-xyz");
        let chat = &config["mcpServers"]["chat"];
        assert_eq!(chat["url"], "http://127.0.0.1:4321/mcp");
        assert_eq!(chat["transport"], "http");
        assert!(chat.get("command").is_none());
    }

    #[test]
    fn build_mcp_config_file_trims_trailing_slash() {
        let config = build_mcp_config_file("http://127.0.0.1:4321/", "tok-xyz");
        assert_eq!(
            config["mcpServers"]["chat"]["url"],
            "http://127.0.0.1:4321/mcp"
        );
    }

    #[test]
    fn build_acp_mcp_servers_http_shape() {
        let servers = build_acp_mcp_servers("http://127.0.0.1:4321", "tok-xyz");
        let arr = servers.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        let entry = &arr[0];
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["name"], "chat");
        assert_eq!(entry["url"], "http://127.0.0.1:4321/mcp");
        assert!(entry["headers"].is_array());
        assert!(entry.get("command").is_none());
    }

    #[test]
    fn build_acp_mcp_servers_trims_trailing_slash() {
        let servers = build_acp_mcp_servers("http://127.0.0.1:4321/", "tok-xyz");
        let arr = servers.as_array().expect("array");
        assert_eq!(arr[0]["url"], "http://127.0.0.1:4321/mcp");
    }
}
