//! OpenCode runtime driver.
//!
//! All shared ACP-native plumbing (reader loop, response routing, session
//! lifecycle, cancel/close, ensure_started) lives in the
//! [`super::acp_native`] module. This file holds only the opencode-specific
//! pieces:
//!
//! - `mcp.chat` config block written into `<wd>/opencode.json` (consumed
//!   from disk by the runtime, NOT through `session/new` params — that's
//!   why `build_session_new_mcp_servers` returns an empty array).
//! - `<wd>/.chorus/opencode-system.md`: the standing system prompt opencode
//!   loads via the `instructions` field of `opencode.json` (per
//!   opencode.ai/docs/en/acp/, ACP mode loads instructions like terminal
//!   mode).
//! - `spawn_opencode`: writes both files and spawns `opencode acp`.
//! - probe + list_models (`opencode models` parsed line-by-line).
//!
//! The previous bootstrap-vs-secondary handle split is gone — the shared
//! `AcpNativeCore::ensure_started` provides the same race-safety guarantee
//! (one spawn, all secondaries wait) without a factory path distinction.

use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{bail, Context};
use async_trait::async_trait;

use crate::agent::AgentRuntime;
use crate::utils::cmd::{command_exists, run_command};

use super::acp_native::{
    self, AcpDriverConfig, AcpNativeCore, InitPromptStrategy, SpawnFut, SpawnedChild,
};
use super::*;

// ---------------------------------------------------------------------------
// MCP config construction
// ---------------------------------------------------------------------------

/// Build the `mcp.chat` config block written into `<wd>/opencode.json`.
///
/// Produces the remote HTTP MCP shape, connecting the runtime to the shared
/// bridge at `{endpoint}/mcp`. This is read from disk by the runtime, NOT
/// passed inline via `session/new` params.
fn build_mcp_chat_config(bridge_endpoint: &str, agent_key: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "remote",
        "url": super::bridge_mcp_url(bridge_endpoint),
        "headers": {
            "X-Agent-Id": agent_key,
        },
    })
}

/// Opencode loads MCP servers from `opencode.json`, not from `session/new`
/// params. Always send an empty array on the wire.
fn build_session_new_mcp_servers(_bridge_endpoint: &str, _agent_key: &str) -> serde_json::Value {
    serde_json::json!([])
}

// ---------------------------------------------------------------------------
// Spawn child
// ---------------------------------------------------------------------------

fn spawn_opencode(spec: Arc<AgentSpec>, key: AgentKey) -> SpawnFut {
    Box::pin(async move {
        let wd = &spec.working_directory;

        // System prompt file. opencode merges every file in `instructions`
        // into the model context at session start (per opencode.ai/config
        // schema — works for every session/new on the same opencode
        // process). Atomic-rename publish so concurrent spawns of the
        // same agent never observe a truncated file.
        let chorus_dir = wd.join(".chorus");
        tokio::fs::create_dir_all(&chorus_dir)
            .await
            .context("failed to create .chorus dir")?;
        let system_md_rel = ".chorus/opencode-system.md";
        let system_md_path = wd.join(system_md_rel);
        let standing_prompt = super::prompt::build_system_prompt(
            &spec,
            &super::prompt::PromptOptions {
                tool_prefix: String::new(),
                extra_critical_rules: vec![
                    "- Do NOT use shell commands to send or receive messages. The MCP tools handle everything.".into(),
                ],
                post_startup_notes: Vec::new(),
                include_stdin_notification_section: false,
                message_notification_style: super::prompt::MessageNotificationStyle::Poll,
            },
        );
        let tmp_system_md = chorus_dir.join(format!(
            "opencode-system.md.{}.{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        tokio::fs::write(&tmp_system_md, standing_prompt)
            .await
            .context("failed to write opencode system.md")?;
        tokio::fs::rename(&tmp_system_md, &system_md_path)
            .await
            .context("failed to publish opencode system.md")?;

        // opencode.json with MCP + model + instructions.
        let model_id = match &spec.reasoning_effort {
            Some(variant) if !variant.is_empty() => {
                format!("{}/{}", spec.model, variant)
            }
            _ => spec.model.clone(),
        };
        let config_path = wd.join("opencode.json");
        let mcp_chat = build_mcp_chat_config(&spec.bridge_endpoint, &key);
        let opencode_config = serde_json::json!({
            "model": model_id,
            "instructions": [system_md_rel],
            "mcp": {
                "chat": mcp_chat,
            }
        });
        let tmp_config = wd.join(format!(
            "opencode.json.{}.{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        tokio::fs::write(&tmp_config, serde_json::to_string_pretty(&opencode_config)?)
            .await
            .context("failed to write opencode.json")?;
        tokio::fs::rename(&tmp_config, &config_path)
            .await
            .context("failed to publish opencode.json")?;

        let mut cmd = Command::new("opencode");
        cmd.arg("acp")
            .current_dir(&spec.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        for ev in &spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let child = cmd.spawn().context("failed to spawn opencode")?;
        Ok(SpawnedChild { child })
    })
}

// ---------------------------------------------------------------------------
// Per-driver static registry + config
// ---------------------------------------------------------------------------

static OPENCODE_REGISTRY: AgentRegistry<AcpNativeCore> = AgentRegistry::new();

static OPENCODE_CFG: AcpDriverConfig = AcpDriverConfig {
    name: "opencode",
    runtime: AgentRuntime::Opencode,
    init_prompt_strategy: InitPromptStrategy::Immediate,
    initialized_notification_payload: None,
    session_load_includes_mcp: false,
    emit_starting_lifecycle: true,
    build_session_new_mcp_servers,
    build_first_prompt_prefix: None,
    spawn_child: spawn_opencode,
    registry: &OPENCODE_REGISTRY,
};

// ---------------------------------------------------------------------------
// Model list parsing
// ---------------------------------------------------------------------------

fn parse_opencode_models(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// OpencodeDriver — thin RuntimeDriver wrapper.
// ---------------------------------------------------------------------------

pub struct OpencodeDriver;

#[async_trait]
impl RuntimeDriver for OpencodeDriver {
    fn runtime(&self) -> AgentRuntime {
        OPENCODE_CFG.runtime
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        if !command_exists("opencode") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::AcpNative,
                capabilities: CapabilitySet::MODEL_LIST,
            });
        }

        let auth = run_command("opencode", &["--version"])
            .ok()
            .map(|result| {
                if result.success {
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
            reason: "opencode does not support login via Chorus".into(),
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        if !command_exists("opencode") {
            return Ok(Vec::new());
        }

        let result = run_command("opencode", &["models"])?;
        if !result.success {
            bail!("opencode: failed to list models: {}", result.stderr.trim());
        }

        Ok(parse_opencode_models(&result.stdout)
            .into_iter()
            .map(ModelInfo::from_id)
            .collect())
    }

    async fn open_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        intent: SessionIntent,
    ) -> anyhow::Result<SessionAttachment> {
        acp_native::open_session(&OPENCODE_CFG, key, spec, intent).await
    }
}

// ---------------------------------------------------------------------------
// Tests — opencode-specific only. Generic ACP-native plumbing tests live
// in `acp_native::tests`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test-opencode".to_string(),
            description: None,
            system_prompt: None,
            model: "openai/gpt-4o".to_string(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_endpoint: "http://127.0.0.1:1".to_string(),
        }
    }

    #[tokio::test]
    async fn test_opencode_driver_probe_not_installed() {
        let driver = OpencodeDriver;
        let probe = driver.probe().await.unwrap();
        if probe.auth == ProbeAuth::NotInstalled {
            assert_eq!(probe.transport, TransportKind::AcpNative);
            assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
        }
    }

    #[tokio::test]
    async fn test_opencode_driver_list_models_not_installed() {
        // When opencode is not on PATH list_models returns empty rather
        // than erroring out. CI environments without opencode rely on this
        // for their probe path to stay clean.
        if command_exists("opencode") {
            return;
        }
        let driver = OpencodeDriver;
        let models = driver.list_models().await.unwrap();
        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn test_opencode_driver_open_session_returns_idle() {
        let driver = OpencodeDriver;
        let key = format!("opencode-agent-idle-{}", uuid::Uuid::new_v4());
        let result = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        assert!(matches!(result.session.process_state(), ProcessState::Idle));
        OPENCODE_REGISTRY.remove(&key);
    }

    #[test]
    fn build_mcp_chat_config_http_shape() {
        let config = build_mcp_chat_config("http://127.0.0.1:4321", "tok-xyz");
        assert_eq!(config["type"], "remote");
        assert_eq!(config["url"], "http://127.0.0.1:4321/mcp");
        assert_eq!(config["headers"]["X-Agent-Id"], "tok-xyz");
    }

    #[test]
    fn build_mcp_chat_config_trims_trailing_slash() {
        let config = build_mcp_chat_config("http://127.0.0.1:4321/", "tok-xyz");
        assert_eq!(config["url"], "http://127.0.0.1:4321/mcp");
    }

    #[test]
    fn parse_opencode_models_strips_blank_lines() {
        let raw = "anthropic/claude-3.5-sonnet\n\n  openai/gpt-4o  \n\n";
        let parsed = parse_opencode_models(raw);
        assert_eq!(parsed, vec!["anthropic/claude-3.5-sonnet", "openai/gpt-4o"]);
    }
}
