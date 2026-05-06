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

use std::path::{Path, PathBuf};
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
    let mut opts = super::prompt::PromptOptions {
        extra_critical_rules: vec![
            "- Do NOT use shell commands to send or receive messages. The MCP tools handle everything.".into(),
        ],
        ..Default::default()
    };
    super::prompt::apply_env_override(&mut opts);
    super::prompt::build_system_prompt(spec, &opts)
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
// Session-file liveness guard
// ---------------------------------------------------------------------------

/// Find the kimi session file that proves this session can actually be
/// resumed via `session/load`. Kimi stores per-session state under
/// `~/.kimi/sessions/<workspace_hash>/<session_id>/`, where
/// `workspace_hash` is a 32-char hex of the working directory.
///
/// We must check for **`context.jsonl`** specifically, not `state.json`.
/// Kimi's `Session.find` (`kimi_cli/session.py`) returns `None` —
/// surfaced to the client as `{"session_id": "Session not found"}`
/// inside an `Invalid params` error — when `context.jsonl` is missing,
/// regardless of any other files in the session directory. `state.json`
/// is written earlier in the session lifecycle, so checking it
/// incorrectly says "alive" for sessions that were started but never
/// completed their first turn (e.g., the trio agent that gets created
/// and immediately stopped before its first prompt lands). A subsequent
/// `session/load` then fails with "Invalid params", and the agent
/// looks broken when the real fix is just to fall back to
/// `session/new` instead.
///
/// Cost is one `read_dir` of `~/.kimi/sessions/` (typically <few
/// hundred entries) plus one `is_file` per workspace until we find a
/// match or exhaust.
fn kimi_session_file(home: &Path, session_id: &str) -> Option<PathBuf> {
    let sessions_dir = home.join(".kimi").join("sessions");
    if !sessions_dir.is_dir() {
        return None;
    }
    let rd = std::fs::read_dir(&sessions_dir).ok()?;
    for entry in rd.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let candidate = entry.path().join(session_id).join("context.jsonl");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Liveness adapter for [`AcpDriverConfig::session_liveness_check`]. Looks
/// up the kimi state file via [`home_dir`] each call.
fn kimi_session_alive(session_id: &str) -> bool {
    kimi_session_file(&home_dir(), session_id).is_some()
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
    session_liveness_check: Some(kimi_session_alive),
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

    // ---- session-file liveness guard ----

    #[test]
    fn kimi_session_file_finds_context_under_workspace_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace_hash = "0749a6673832efdcb848ee54b8d3a625";
        let sid = "d6c7d632-ecdd-486a-96b1-df0091c6550d";
        let session_dir = tmp
            .path()
            .join(".kimi")
            .join("sessions")
            .join(workspace_hash)
            .join(sid);
        std::fs::create_dir_all(&session_dir).unwrap();
        let context = session_dir.join("context.jsonl");
        std::fs::write(&context, b"").unwrap();

        let found = kimi_session_file(tmp.path(), sid).expect("context.jsonl should be found");
        assert_eq!(found, context);
    }

    #[test]
    fn kimi_session_file_walks_multiple_workspace_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join(".kimi").join("sessions");
        // Five decoy workspace dirs without our session, then the real one.
        for hex in ["a1b2", "c3d4", "e5f6", "0011", "2233"] {
            std::fs::create_dir_all(sessions.join(hex)).unwrap();
        }
        let real_workspace = sessions.join("ffff");
        std::fs::create_dir_all(&real_workspace).unwrap();
        let sid = "real-session-uuid";
        let context = real_workspace.join(sid).join("context.jsonl");
        std::fs::create_dir_all(context.parent().unwrap()).unwrap();
        std::fs::write(&context, b"").unwrap();

        let found = kimi_session_file(tmp.path(), sid).expect("context.jsonl should be found");
        assert_eq!(found, context);
    }

    #[test]
    fn kimi_session_file_returns_none_when_only_state_json_exists() {
        // Regression for the bug fixed by this change: kimi 1.41 rejects
        // `session/load` for sessions whose `context.jsonl` is missing,
        // even when other files (like `state.json`) are present. The
        // liveness probe must mirror kimi's `Session.find` criterion.
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp
            .path()
            .join(".kimi")
            .join("sessions")
            .join("aabbcc")
            .join("sid-only-state");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("state.json"), b"{}").unwrap();

        assert!(
            kimi_session_file(tmp.path(), "sid-only-state").is_none(),
            "sessions with only state.json (no context.jsonl) must NOT be considered resumable"
        );
    }

    #[test]
    fn kimi_session_file_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".kimi").join("sessions")).unwrap();
        assert!(kimi_session_file(tmp.path(), "no-such-session").is_none());
    }

    #[test]
    fn kimi_session_file_returns_none_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(kimi_session_file(tmp.path(), "anything").is_none());
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
