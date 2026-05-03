//! Gemini runtime driver.
//!
//! All shared ACP-native plumbing (reader loop, response routing, session
//! lifecycle, cancel/close, ensure_started) lives in the
//! [`super::acp_native`] module. This file holds only the gemini-specific
//! pieces:
//!
//! - `mcpServers` JSON shape sent in `session/new` params.
//! - `ensure_gemini_system_md`: writes the Gemini system prompt file
//!   (`<wd>/.chorus/gemini-system.md`) by capturing the runtime's built-in
//!   baseline + appending Chorus's standing prompt, returning the absolute
//!   path passed via `GEMINI_SYSTEM_MD`.
//! - `build_gemini_command`: assembles `gemini --acp [--model X]` with
//!   `current_dir` set, NOT `--work-dir` (Gemini CLI 0.38.x rejects that
//!   flag).
//! - `spawn_gemini`: writes the system.md, builds the command, spawns.
//! - `initialized` JSON-RPC notification sent post-init (Gemini ACP
//!   requires it).
//! - probe + list_models.

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

fn build_acp_mcp_servers(bridge_endpoint: &str, agent_key: &str) -> serde_json::Value {
    let url = super::bridge_mcp_url(bridge_endpoint);
    serde_json::json!([{
        "type": "http",
        "name": "chat",
        "url": url,
        "headers": [{"name":"X-Agent-Id","value":agent_key}]
    }])
}

// ---------------------------------------------------------------------------
// Gemini system prompt cache
// ---------------------------------------------------------------------------

const GEMINI_CHORUS_SUBDIR: &str = ".chorus";
const GEMINI_BASELINE_FILE: &str = "gemini-baseline.md";
const GEMINI_SYSTEM_FILE: &str = "gemini-system.md";

/// Write `<wd>/.chorus/gemini-system.md` containing Gemini's built-in
/// baseline system prompt followed by Chorus's standing prompt, returning
/// the absolute path. The path is consumed via `GEMINI_SYSTEM_MD` on spawn.
///
/// `GEMINI_SYSTEM_MD` is a *full replacement* for Gemini's built-in prompt
/// (per Gemini docs), so the baseline must be included or Gemini loses its
/// safety / approval / tool-use rules. The baseline is captured on first
/// spawn via `GEMINI_WRITE_SYSTEM_MD=<path> gemini -p ping` and cached in
/// the agent's workspace; subsequent spawns reuse it.
///
/// Both writes use atomic-rename so concurrent spawns of the same agent
/// can never observe a truncated file. The baseline subprocess is run
/// against a per-spawn temp path before atomic-renaming into place, so
/// concurrent first-spawns may both invoke `gemini -p ping` (cheap,
/// idempotent) but the final file is intact whichever caller wins the
/// rename.
async fn ensure_gemini_system_md(spec: &AgentSpec) -> anyhow::Result<std::path::PathBuf> {
    let chorus_dir = spec.working_directory.join(GEMINI_CHORUS_SUBDIR);
    tokio::fs::create_dir_all(&chorus_dir)
        .await
        .context("failed to create .chorus dir")?;
    let baseline_path = chorus_dir.join(GEMINI_BASELINE_FILE);
    let system_path = chorus_dir.join(GEMINI_SYSTEM_FILE);

    if !baseline_path.exists() {
        let tmp_baseline = chorus_dir.join(format!(
            "{}.{}.{}.tmp",
            GEMINI_BASELINE_FILE,
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        let status = tokio::process::Command::new("gemini")
            .arg("-p")
            .arg("ping")
            .arg("--skip-trust")
            .env("GEMINI_WRITE_SYSTEM_MD", &tmp_baseline)
            .env("GEMINI_CLI_TRUST_WORKSPACE", "true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .context("failed to invoke `gemini` to capture baseline system prompt")?;
        if !status.success() || !tmp_baseline.exists() {
            let _ = tokio::fs::remove_file(&tmp_baseline).await;
            anyhow::bail!(
                "gemini baseline capture failed (status {status}); \
                 ensure `gemini` is installed and authenticated"
            );
        }
        // Atomic publish. If a sibling caller raced and already wrote the
        // baseline, our rename overwrites it with identical content.
        tokio::fs::rename(&tmp_baseline, &baseline_path)
            .await
            .context("failed to publish gemini baseline")?;
    }

    let baseline = tokio::fs::read_to_string(&baseline_path)
        .await
        .context("failed to read gemini baseline")?;
    let standing = super::prompt::build_system_prompt(
        spec,
        &super::prompt::PromptOptions {
            extra_critical_rules: vec![
                "- Do NOT use shell commands to send or receive messages. The MCP tools handle everything.".into(),
            ],
            ..Default::default()
        },
    );
    let tmp_system = chorus_dir.join(format!(
        "{}.{}.{}.tmp",
        GEMINI_SYSTEM_FILE,
        std::process::id(),
        uuid::Uuid::new_v4().simple(),
    ));
    tokio::fs::write(&tmp_system, format!("{baseline}\n\n---\n\n{standing}"))
        .await
        .context("failed to write gemini system.md")?;
    tokio::fs::rename(&tmp_system, &system_path)
        .await
        .context("failed to publish gemini system.md")?;

    tokio::fs::canonicalize(&system_path)
        .await
        .context("failed to canonicalize gemini system.md path")
}

fn build_gemini_command(spec: &AgentSpec, system_md: &std::path::Path) -> Command {
    let mut args = vec!["--acp".to_string()];
    if !spec.model.is_empty() {
        args.push("--model".to_string());
        args.push(spec.model.clone());
    }

    let mut cmd = Command::new("gemini");
    cmd.args(&args)
        .current_dir(&spec.working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("FORCE_COLOR", "0")
        .env("NO_COLOR", "1")
        .env("GEMINI_SYSTEM_MD", system_md)
        .env("GEMINI_CLI_TRUST_WORKSPACE", "true");
    for ev in &spec.env_vars {
        cmd.env(&ev.key, &ev.value);
    }
    cmd
}

// ---------------------------------------------------------------------------
// Spawn child
// ---------------------------------------------------------------------------

fn spawn_gemini(spec: Arc<AgentSpec>, _key: AgentKey) -> SpawnFut {
    Box::pin(async move {
        let system_md = ensure_gemini_system_md(&spec).await?;
        let mut cmd = build_gemini_command(&spec, &system_md);
        let child = cmd.spawn().context("failed to spawn gemini")?;
        Ok(SpawnedChild { child })
    })
}

// ---------------------------------------------------------------------------
// Session-file liveness guard
// ---------------------------------------------------------------------------

/// Find a gemini session-state file for `session_id`. Gemini stores per-
/// session state under
/// `~/.gemini/tmp/<project_key>/chats/session-<datetime>-<short_id>.jsonl`,
/// where `<project_key>` is derived from the working dir (typically the
/// basename) and `<short_id>` is the first 8 hex chars of the full UUID.
/// Walks every project-key dir under `~/.gemini/tmp/` and matches by
/// filename suffix. Returns the first match, or `None` if no live session
/// file exists.
///
/// Mirrors the codex/opencode/kimi/claude file-existence pattern. Cost is
/// one `read_dir` of `~/.gemini/tmp/` (typically <100 entries) plus a
/// per-project `read_dir` of `chats/` until match.
fn gemini_session_file(home: &Path, session_id: &str) -> Option<PathBuf> {
    let tmp_dir = home.join(".gemini").join("tmp");
    if !tmp_dir.is_dir() {
        return None;
    }
    // Gemini truncates the UUID to its first 8 hex chars in the filename.
    // If the caller passes a shorter id, anchor on what we have; longer
    // ids are truncated to the same prefix gemini would use.
    let needle: String = session_id
        .chars()
        .take(8)
        .collect::<String>()
        .to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let suffix = format!("-{needle}.jsonl");
    let suffix_old = format!("-{needle}.json");

    let Ok(rd) = std::fs::read_dir(&tmp_dir) else {
        return None;
    };
    for project_entry in rd.flatten() {
        if !project_entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let chats_dir = project_entry.path().join("chats");
        let Ok(chats_rd) = std::fs::read_dir(&chats_dir) else {
            continue;
        };
        for chat_entry in chats_rd.flatten() {
            let name = chat_entry.file_name();
            let name_str = name.to_string_lossy().to_lowercase();
            if name_str.starts_with("session-")
                && (name_str.ends_with(&suffix) || name_str.ends_with(&suffix_old))
            {
                return Some(chat_entry.path());
            }
        }
    }
    None
}

/// Liveness adapter for [`AcpDriverConfig::session_liveness_check`]. Looks
/// up the gemini session file via [`home_dir`] each call.
fn gemini_session_alive(session_id: &str) -> bool {
    gemini_session_file(&home_dir(), session_id).is_some()
}

// ---------------------------------------------------------------------------
// Per-driver static registry + config
// ---------------------------------------------------------------------------

static GEMINI_REGISTRY: AgentRegistry<AcpNativeCore> = AgentRegistry::new();

/// Gemini ACP requires the `initialized` notification after the
/// `initialize` response (per Gemini's ACP server implementation). The
/// shared reader sends this on init-response receipt.
const GEMINI_INITIALIZED_NOTIFICATION: &str =
    r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;

static GEMINI_CFG: AcpDriverConfig = AcpDriverConfig {
    name: "gemini",
    runtime: AgentRuntime::Gemini,
    init_prompt_strategy: InitPromptStrategy::Immediate,
    initialized_notification_payload: Some(GEMINI_INITIALIZED_NOTIFICATION),
    session_load_includes_mcp: false,
    emit_starting_lifecycle: false,
    build_session_new_mcp_servers: build_acp_mcp_servers,
    build_first_prompt_prefix: None,
    spawn_child: spawn_gemini,
    registry: &GEMINI_REGISTRY,
    session_liveness_check: Some(gemini_session_alive),
};

// ---------------------------------------------------------------------------
// GeminiDriver — thin RuntimeDriver wrapper.
// ---------------------------------------------------------------------------

pub struct GeminiDriver;

#[async_trait]
impl RuntimeDriver for GeminiDriver {
    fn runtime(&self) -> AgentRuntime {
        GEMINI_CFG.runtime
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        if !command_exists("gemini") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::AcpNative,
                capabilities: CapabilitySet::MODEL_LIST,
            });
        }

        // Check GEMINI_API_KEY env var first.
        if std::env::var("GEMINI_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::Authed,
                transport: TransportKind::AcpNative,
                capabilities: CapabilitySet::MODEL_LIST,
            });
        }

        // OAuth personal account: check for ~/.gemini/oauth_creds.json
        let home = home_dir();
        let auth = read_file(&home.join(".gemini/oauth_creds.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .map(|payload| {
                let has_token = payload["access_token"]
                    .as_str()
                    .is_some_and(|v| !v.trim().is_empty());
                if has_token {
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
            reason: "gemini does not support login via Chorus".into(),
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo::from_id("auto-gemini-3".into()),
            ModelInfo::from_id("gemini-3.1-pro-preview".into()),
            ModelInfo::from_id("gemini-3-flash-preview".into()),
            ModelInfo::from_id("gemini-3.1-flash-lite-preview".into()),
            ModelInfo::from_id("gemini-2.5-pro".into()),
            ModelInfo::from_id("gemini-2.5-flash".into()),
            ModelInfo::from_id("gemini-2.5-flash-lite".into()),
        ])
    }

    async fn open_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        intent: SessionIntent,
    ) -> anyhow::Result<SessionAttachment> {
        acp_native::open_session(&GEMINI_CFG, key, spec, intent).await
    }
}

// ---------------------------------------------------------------------------
// Tests — gemini-specific only. Generic ACP-native plumbing tests live in
// `acp_native::tests`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test".into(),
            description: None,
            system_prompt: None,
            model: "gemini-3.1-pro-preview".into(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: std::env::temp_dir(),
            bridge_endpoint: "http://127.0.0.1:9999".into(),
        }
    }

    #[test]
    fn gemini_runtime_variant_parses() {
        assert_eq!(AgentRuntime::parse("gemini"), Some(AgentRuntime::Gemini));
        assert_eq!(AgentRuntime::Gemini.as_str(), "gemini");
    }

    // ---- session-file liveness guard ----

    #[test]
    fn gemini_session_file_finds_jsonl_by_short_id() {
        let tmp = tempfile::tempdir().unwrap();
        let chats = tmp
            .path()
            .join(".gemini")
            .join("tmp")
            .join("agent-1")
            .join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        // Real gemini-style filename: session-<datetime>-<8-hex>.jsonl
        let f = chats.join("session-2026-05-02T05-01-7dd1c95e.jsonl");
        std::fs::write(
            &f,
            br#"{"sessionId":"7dd1c95e-314c-4905-992a-afccf2516ae9"}"#,
        )
        .unwrap();

        let full_id = "7dd1c95e-314c-4905-992a-afccf2516ae9";
        let found = gemini_session_file(tmp.path(), full_id).expect("session file should be found");
        assert_eq!(found, f);
    }

    #[test]
    fn gemini_session_file_walks_multiple_project_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_dir = tmp.path().join(".gemini").join("tmp");
        // Decoy projects with unrelated sessions
        for proj in ["chorus", "design-reviews", "experiments"] {
            let chats = tmp_dir.join(proj).join("chats");
            std::fs::create_dir_all(&chats).unwrap();
            std::fs::write(chats.join("session-2026-04-01T00-00-decoyabc.jsonl"), b"{}").unwrap();
        }
        // Real session in a fourth project
        let target_chats = tmp_dir.join("real-agent").join("chats");
        std::fs::create_dir_all(&target_chats).unwrap();
        let target = target_chats.join("session-2026-05-02T05-01-7dd1c95e.jsonl");
        std::fs::write(&target, b"{}").unwrap();

        let found = gemini_session_file(tmp.path(), "7dd1c95e-314c-4905-992a-afccf2516ae9")
            .expect("should walk to real-agent and find target");
        assert_eq!(found, target);
    }

    #[test]
    fn gemini_session_file_accepts_old_json_extension() {
        // Older gemini sessions used .json (no `l`); we accept both.
        let tmp = tempfile::tempdir().unwrap();
        let chats = tmp
            .path()
            .join(".gemini")
            .join("tmp")
            .join("legacy")
            .join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        let f = chats.join("session-2026-04-23T08-29-167fc060.json");
        std::fs::write(&f, b"{}").unwrap();
        let found = gemini_session_file(tmp.path(), "167fc060-dae8-4632-a718-cf92fc90bd2f")
            .expect("legacy .json filenames must still be detected");
        assert_eq!(found, f);
    }

    #[test]
    fn gemini_session_file_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            tmp.path()
                .join(".gemini")
                .join("tmp")
                .join("agent")
                .join("chats"),
        )
        .unwrap();
        assert!(gemini_session_file(tmp.path(), "deadbeef-1111-2222-3333-444444444444").is_none());
    }

    #[test]
    fn gemini_session_file_returns_none_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(gemini_session_file(tmp.path(), "anything").is_none());
    }

    #[test]
    fn build_gemini_command_uses_current_dir_not_work_dir_flag() {
        let spec = test_spec();
        let cmd = build_gemini_command(&spec, std::path::Path::new("/tmp/dummy-system.md"));
        let args: Vec<_> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(args, vec!["--acp", "--model", "gemini-3.1-pro-preview"]);
        assert_eq!(
            cmd.get_current_dir(),
            Some(spec.working_directory.as_path())
        );
        assert!(
            !args.iter().any(|arg| arg == "--work-dir"),
            "Gemini CLI 0.38.x rejects --work-dir; use process current_dir instead"
        );
    }

    #[tokio::test]
    async fn probe_returns_not_installed_when_binary_missing() {
        let driver = GeminiDriver;
        let probe = driver.probe().await.expect("probe should not panic");
        assert_eq!(probe.transport, TransportKind::AcpNative);
        assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
    }

    #[tokio::test]
    async fn list_models_returns_gemini_models() {
        let driver = GeminiDriver;
        let models = driver
            .list_models()
            .await
            .expect("list_models should succeed");
        let ids: Vec<_> = models.into_iter().map(|m| m.id).collect();
        assert!(ids.contains(&"gemini-3.1-pro-preview".to_string()));
        assert!(ids.contains(&"gemini-2.5-pro".to_string()));
    }

    #[tokio::test]
    async fn login_returns_failed() {
        let driver = GeminiDriver;
        match driver.login().await.expect("login should return") {
            LoginOutcome::Failed { reason } => {
                assert!(reason.contains("does not support login"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_sessions_returns_empty() {
        let driver = GeminiDriver;
        let sessions = driver
            .list_sessions()
            .await
            .expect("list_sessions should succeed");
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn event_fan_out_forwards_events() {
        let (stream, tx) = EventFanOut::new();
        let mut rx = stream.subscribe();

        tx.send(DriverEvent::Lifecycle {
            key: "a1".into(),
            state: ProcessState::Idle,
        })
        .await
        .unwrap();

        let got = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        match got {
            DriverEvent::Lifecycle { key, .. } => assert_eq!(key, "a1"),
            other => panic!("expected Lifecycle, got {other:?}"),
        }
    }

    /// `open_session` returns an Idle handle without spawning the runtime.
    /// Confirms the driver wiring delegates to `acp_native::open_session`.
    #[tokio::test]
    async fn open_session_returns_idle() {
        let driver = GeminiDriver;
        let key = format!("gemini-agent-idle-{}", uuid::Uuid::new_v4());
        let result = driver
            .open_session(key.clone(), test_spec(), SessionIntent::New)
            .await
            .unwrap();
        assert!(matches!(result.session.process_state(), ProcessState::Idle));
        GEMINI_REGISTRY.remove(&key);
    }
}
