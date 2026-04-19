//! Live multi-session integration tests for the four real runtime drivers.
//!
//! These tests exist to prove that the wire-protocol assumptions made by the
//! `new_session` / `resume_session` work landed in PR #66 (the multi-session
//! driver work) hold against **real** runtime binaries — not just the fake
//! transports used in the unit tests. They exercise:
//!
//! - **Codex**: whether a second `thread/start` on an already-running
//!   `codex app-server` process mints a distinct thread id (the "one app-server
//!   process, N threads" assumption documented at
//!   <https://developers.openai.com/codex/app-server>).
//! - **Kimi / OpenCode**: whether `session/new` on an already-initialized ACP
//!   connection mints a distinct `sessionId` (the assumption documented at
//!   <https://agentclientprotocol.com/protocol/session-setup>).
//! - **Claude**: whether `claude -p --resume <id>` actually continues the
//!   named session — and whether spawning a second `claude -p` alongside a
//!   live bootstrap preserves both.
//!
//! Each runtime has two tests:
//!   1. *bootstrap + secondary round-trip* — attach, start the bootstrap, open
//!      a secondary via `new_session`, assert distinct session ids, close the
//!      bootstrap, and assert the secondary is still usable.
//!   2. *resume round-trip* (Claude + Codex only) — attach, start, capture the
//!      session id, close, then `resume_session` in a fresh driver attach and
//!      confirm the id round-trips.
//!
//! # Running
//!
//! All tests are `#[ignore]` by default — `cargo test` skips them. Run
//! explicitly:
//!
//! ```bash
//! cargo test --test live_multi_session_tests -- --ignored --nocapture
//! ```
//!
//! Per-runtime single test:
//!
//! ```bash
//! cargo test --test live_multi_session_tests codex_multi_session_bootstrap -- --ignored --nocapture
//! ```
//!
//! If the required binary is missing on `PATH` OR required auth is missing, the
//! test prints a `SKIP:` message and returns `Ok(())` rather than failing. This
//! mirrors the pattern in `tests/live_runtime_tests.rs`.
//!
//! # Required environment
//!
//! | Test                                                          | Binary    | Auth                                                         | Model env var |
//! |---------------------------------------------------------------|-----------|--------------------------------------------------------------|---------------|
//! | `opencode_multi_session_bootstrap_close_preserves_secondary`  | `opencode`| `opencode auth login`                                        | `OPENCODE_MODEL` (default `opencode/gpt-5-nano`) |
//! | `claude_multi_session_bootstrap_close_preserves_secondary`    | `claude`  | `ANTHROPIC_API_KEY` or `claude login`                        | `CHORUS_TEST_CLAUDE_MODEL` (default `sonnet`) |
//! | `codex_multi_session_bootstrap_close_preserves_secondary`     | `codex`   | `OPENAI_API_KEY` or `codex login`                            | `CHORUS_TEST_CODEX_MODEL` (default `gpt-5.4`) |
//! | `kimi_multi_session_bootstrap_close_preserves_secondary`      | `kimi`    | `~/.kimi/credentials/kimi-code.json`                         | `CHORUS_TEST_KIMI_MODEL` (default `kimi-code/kimi-for-coding`) |
//! | `codex_multi_session_resume_preserves_thread_id`              | `codex`   | same as above                                                | same as above |
//! | `claude_multi_session_resume_preserves_session_id`            | `claude`  | same as above                                                | same as above |
//!
//! # What these tests do NOT do
//!
//! They deliberately avoid sending expensive LLM prompts. Most paths just
//! exercise session lifecycle + session_id routing (which costs zero tokens).
//! Only the Claude tests send a trivial prompt, because `claude -p` in
//! stream-json mode does not emit a `system/init` frame (and thus no session
//! id) until stdin delivers at least one message — so we send `"ok"` to get
//! the session id, then close. The other runtimes emit SessionAttached during
//! their native handshake without any prompt.
//!
//! # Debugging
//!
//! Use `--nocapture` to see runtime stderr piped through our tracing setup. If
//! a test hangs, check whether the runtime has valid auth in its config dir.
//! Re-run with `RUST_LOG=chorus=debug` to see the per-driver handshake traces.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use chorus::agent::drivers::claude::ClaudeDriver;
use chorus::agent::drivers::codex::CodexDriver;
use chorus::agent::drivers::kimi::KimiDriver;
use chorus::agent::drivers::opencode::OpencodeDriver;
use chorus::agent::drivers::{
    AgentKey, AgentSpec, DriverEvent, EventStreamHandle, PromptReq, RuntimeDriver, SessionId,
    StartOpts,
};
use chorus::agent::runtime_status::{SharedRuntimeStatusProvider, SystemRuntimeStatusProvider};
use chorus::agent::AgentLifecycle;
use chorus::bridge::serve::build_bridge_router;
use chorus::server::build_router_with_services;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{ReceivedMessage, SenderType};
use chorus::store::{AgentRecordUpsert, Store};
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

// ---------------------------------------------------------------------------
// Shared harness — intentionally duplicates the style of live_runtime_tests.rs
// rather than extracting into a shared module. Cargo integration tests don't
// share modules across files without extra wiring, and this file is the only
// other consumer of these helpers.
// ---------------------------------------------------------------------------

/// No-op lifecycle used when running the Chorus server in-process for tests.
struct NoopLifecycle;

impl AgentLifecycle for NoopLifecycle {
    fn start_agent<'a>(
        &'a self,
        _agent_name: &'a str,
        _wake_message: Option<ReceivedMessage>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn notify_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn stop_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn get_activity_log_data(
        &self,
        _agent_name: &str,
        _after_seq: Option<u64>,
    ) -> chorus::agent::activity_log::ActivityLogResponse {
        chorus::agent::activity_log::ActivityLogResponse {
            entries: vec![],
            agent_activity: "offline".to_string(),
            agent_detail: String::new(),
        }
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        vec![]
    }
}

/// Start a Chorus server in-process with an in-memory SQLite store. Returns
/// the server's base URL and the shared `Store`.
async fn start_chorus_server() -> anyhow::Result<(String, Arc<Store>)> {
    let store = Arc::new(Store::open(":memory:")?);
    store.create_human("tester")?;
    store.create_channel("general", Some("General"), ChannelType::Channel)?;
    store.join_channel("general", "tester", SenderType::Human)?;

    let router = build_router_with_services(
        store.clone(),
        Arc::new(NoopLifecycle),
        Arc::new(SystemRuntimeStatusProvider::new(
            chorus::agent::manager::build_driver_registry(),
        )) as SharedRuntimeStatusProvider,
        vec![],
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok((url, store))
}

/// Start the shared bridge pointing at the given Chorus server URL. Returns
/// the bridge base URL and a cancellation token for shutdown.
async fn start_bridge_with_server(
    server_url: &str,
) -> anyhow::Result<(String, tokio_util::sync::CancellationToken)> {
    let (app, ct) = build_bridge_router(server_url);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let addr = format!("http://127.0.0.1:{}", port);

    let shutdown_ct = ct.clone();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown_ct.cancelled().await })
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    Ok((addr, ct))
}

/// Check whether a binary exists on `PATH` by invoking it with `--version`.
fn binary_on_path(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Bundle of environment prerequisites for a test run. If any field is empty
/// the caller should print `SKIP:` and return cleanly.
#[allow(dead_code)]
struct LiveEnv {
    server_url: String,
    store: Arc<Store>,
    bridge_url: String,
    bridge_ct: tokio_util::sync::CancellationToken,
    tmpdir: tempfile::TempDir,
}

/// Spin up Chorus server + bridge + a working-dir tempdir. The caller is
/// responsible for calling `bridge_ct.cancel()` during cleanup.
async fn make_live_env() -> anyhow::Result<LiveEnv> {
    let (server_url, store) = start_chorus_server().await?;
    let (bridge_url, bridge_ct) = start_bridge_with_server(&server_url).await?;
    let tmpdir = tempfile::tempdir()?;
    Ok(LiveEnv {
        server_url,
        store,
        bridge_url,
        bridge_ct,
        tmpdir,
    })
}

/// Seed an agent record and join it to `#general`. Required before the
/// bridge will accept messages from this agent.
fn seed_agent(
    store: &Store,
    agent_key: &str,
    display_name: &str,
    runtime: &str,
    model: &str,
) -> anyhow::Result<()> {
    store.create_agent_record(&AgentRecordUpsert {
        name: agent_key,
        display_name,
        description: None,
        system_prompt: None,
        runtime,
        model,
        reasoning_effort: None,
        env_vars: &[],
    })?;
    store.join_channel("general", agent_key, SenderType::Agent)?;
    Ok(())
}

/// Build an `AgentSpec` wired to the bridge + working dir for a test.
fn make_spec(display_name: &str, model: &str, env: &LiveEnv) -> AgentSpec {
    AgentSpec {
        display_name: display_name.to_string(),
        description: None,
        system_prompt: None,
        model: model.to_string(),
        reasoning_effort: None,
        env_vars: vec![],
        working_directory: env.tmpdir.path().to_path_buf(),
        bridge_endpoint: env.bridge_url.clone(),
    }
}

/// Wait until a `DriverEvent::SessionAttached` for the given key arrives on
/// the stream. Returns the session id. Bounded by `deadline`.
///
/// If the deadline fires before any SessionAttached for this key, returns a
/// descriptive error listing the events that *did* arrive.
async fn await_session_attached(
    rx: &mut Receiver<DriverEvent>,
    wait_for: Duration,
    key: &AgentKey,
) -> anyhow::Result<SessionId> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + wait_for;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!(
                "timed out after {:?} waiting for SessionAttached(key={key}); events received: {:?}",
                wait_for,
                seen
            ));
        }
        match timeout(remaining, rx.recv()).await {
            Ok(Some(ev)) => match ev {
                DriverEvent::SessionAttached {
                    key: ev_key,
                    session_id,
                } if ev_key == *key => return Ok(session_id),
                DriverEvent::Failed { error, .. } => {
                    return Err(anyhow!(
                        "driver emitted Failed while waiting for SessionAttached: {:?}; prior events: {:?}",
                        error,
                        seen
                    ));
                }
                other => seen.push(debug_event_kind(&other)),
            },
            Ok(None) => {
                return Err(anyhow!(
                    "event stream closed while waiting for SessionAttached; prior events: {:?}",
                    seen
                ));
            }
            Err(_) => {
                return Err(anyhow!(
                    "timed out after {:?} waiting for SessionAttached(key={key}); events received: {:?}",
                    wait_for,
                    seen
                ));
            }
        }
    }
}

/// Kind-only debug summary for an event, so failure messages don't dump full
/// payloads.
fn debug_event_kind(e: &DriverEvent) -> String {
    match e {
        DriverEvent::Lifecycle { state, .. } => format!("Lifecycle({:?})", state),
        DriverEvent::SessionAttached { session_id, .. } => {
            format!("SessionAttached({session_id})")
        }
        DriverEvent::Output { .. } => "Output".to_string(),
        DriverEvent::Completed { .. } => "Completed".to_string(),
        DriverEvent::Failed { error, .. } => format!("Failed({error:?})"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Codex: attach + start (no prompt) → `SessionAttached(s1)`.
/// `new_session` + start → `SessionAttached(s2)` with s2 ≠ s1.
/// Close the bootstrap → secondary must still report its session id.
///
/// This is the core multi-thread-in-one-process test for the Codex app-server
/// transport.
#[tokio::test]
#[ignore = "requires codex binary + OpenAI auth (OPENAI_API_KEY or codex login)"]
async fn codex_multi_session_bootstrap_close_preserves_secondary() -> anyhow::Result<()> {
    if !binary_on_path("codex") {
        eprintln!("SKIP: `codex` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_CODEX_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    let env = make_live_env().await?;
    let agent_key = "codex-multi-bot".to_string();
    seed_agent(&env.store, &agent_key, "Codex Multi Bot", "codex", &model)?;
    let spec = make_spec("Codex Multi Bot", &model, &env);

    let driver = CodexDriver;
    let attach = driver.attach(agent_key.clone(), spec.clone()).await?;
    let events: EventStreamHandle = attach.events.clone();
    let mut rx = events.subscribe();
    let mut bootstrap = attach.handle;

    // Guard spawned processes so Drop tears them down even on panic.
    let outcome = async {
        bootstrap.start(StartOpts::default(), None).await?;
        let s1 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("bootstrap SessionAttached")?;

        let secondary_attach = driver.new_session(agent_key.clone(), spec.clone()).await?;
        let mut secondary = secondary_attach.handle;
        secondary.start(StartOpts::default(), None).await?;
        let s2 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("secondary SessionAttached")?;

        assert_ne!(s1, s2, "codex new_session must mint a distinct thread id");

        // Close the bootstrap. Secondary must remain live.
        bootstrap.close().await?;

        // Secondary handle must still report its session id.
        assert_eq!(
            secondary.session_id(),
            Some(s2.as_str()),
            "secondary session_id must survive bootstrap close"
        );

        // Probe: mint a tertiary via `new_session` — this only works if the
        // codex app-server child and its reader loop survived the bootstrap
        // close. `resume_session(s2)` would be a more direct liveness probe,
        // but real codex rejects `thread/resume` on a thread that has never
        // run a turn with `-32600 "no rollout found for thread id <id>"` (see
        // `codex_multi_session_resume_turnless_thread_surfaces_error` for the
        // driver-side coverage of that path). Minting a fresh thread via
        // `thread/start` requires zero rollout state and zero LLM tokens,
        // which makes this the cheapest "the shared process is still alive"
        // check we can run.
        let tertiary_attach = driver.new_session(agent_key.clone(), spec.clone()).await?;
        let mut tertiary = tertiary_attach.handle;
        tertiary.start(StartOpts::default(), None).await?;
        let s3 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("tertiary SessionAttached after bootstrap close")?;
        assert_ne!(s3, s1, "tertiary thread id must differ from bootstrap");
        assert_ne!(s3, s2, "tertiary thread id must differ from secondary");
        tertiary.close().await?;

        // Close the secondary → registry prunes → a fresh attach on a new
        // key must succeed and spawn a fresh process.
        secondary.close().await?;

        let fresh_key = "codex-multi-bot-fresh".to_string();
        seed_agent(&env.store, &fresh_key, "Codex Multi Bot Fresh", "codex", &model)?;
        let fresh = driver.attach(fresh_key.clone(), spec.clone()).await?;
        let mut fresh_rx = fresh.events.subscribe();
        let mut fresh_handle = fresh.handle;
        fresh_handle.start(StartOpts::default(), None).await?;
        let _ = await_session_attached(&mut fresh_rx, Duration::from_secs(30), &fresh_key)
            .await
            .context("fresh attach SessionAttached")?;
        fresh_handle.close().await?;

        Ok::<_, anyhow::Error>(())
    }
    .await;

    env.bridge_ct.cancel();
    outcome
}

/// Kimi: attach + start (no prompt) → `SessionAttached(s1)`.
/// `new_session` + start → `SessionAttached(s2)` with s2 ≠ s1.
/// Close the bootstrap → secondary must still report its session id.
#[tokio::test]
#[ignore = "requires kimi binary + Moonshot auth (~/.kimi/credentials/kimi-code.json)"]
async fn kimi_multi_session_bootstrap_close_preserves_secondary() -> anyhow::Result<()> {
    if !binary_on_path("kimi") {
        eprintln!("SKIP: `kimi` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_KIMI_MODEL")
        .unwrap_or_else(|_| "kimi-code/kimi-for-coding".to_string());
    let env = make_live_env().await?;
    let agent_key = "kimi-multi-bot".to_string();
    seed_agent(&env.store, &agent_key, "Kimi Multi Bot", "kimi", &model)?;
    let spec = make_spec("Kimi Multi Bot", &model, &env);

    let driver = KimiDriver;
    let attach = driver.attach(agent_key.clone(), spec.clone()).await?;
    let events = attach.events.clone();
    let mut rx = events.subscribe();
    let mut bootstrap = attach.handle;

    let outcome = async {
        bootstrap.start(StartOpts::default(), None).await?;
        let s1 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("bootstrap SessionAttached (kimi warmup)")?;

        let secondary_attach = driver.new_session(agent_key.clone(), spec.clone()).await?;
        let mut secondary = secondary_attach.handle;
        secondary.start(StartOpts::default(), None).await?;
        let s2 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("secondary SessionAttached")?;

        assert_ne!(s1, s2, "kimi session/new must mint a distinct session id");
        assert_eq!(
            secondary.session_id(),
            Some(s2.as_str()),
            "secondary local session_id matches emitted SessionAttached"
        );

        bootstrap.close().await?;

        assert_eq!(
            secondary.session_id(),
            Some(s2.as_str()),
            "secondary session_id must survive bootstrap close"
        );

        // Confirm the child process + ACP connection are still alive by
        // opening a third session via new_session on the SAME key. This
        // only works if the driver's shared state persisted past the
        // bootstrap close.
        let tertiary_attach = driver.new_session(agent_key.clone(), spec.clone()).await?;
        let mut tertiary = tertiary_attach.handle;
        tertiary.start(StartOpts::default(), None).await?;
        let s3 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("tertiary SessionAttached after bootstrap close")?;
        assert_ne!(s3, s2);
        assert_ne!(s3, s1);

        tertiary.close().await?;
        secondary.close().await?;

        Ok::<_, anyhow::Error>(())
    }
    .await;

    env.bridge_ct.cancel();
    outcome
}

/// OpenCode: attach + start (no prompt) → `SessionAttached(s1)`.
/// `new_session` + start → `SessionAttached(s2)` with s2 ≠ s1.
/// Close the bootstrap → secondary survives.
#[tokio::test]
#[ignore = "requires opencode binary + login (opencode auth login)"]
async fn opencode_multi_session_bootstrap_close_preserves_secondary() -> anyhow::Result<()> {
    if !binary_on_path("opencode") {
        eprintln!("SKIP: `opencode` binary not found on PATH");
        return Ok(());
    }

    let model =
        std::env::var("OPENCODE_MODEL").unwrap_or_else(|_| "opencode/gpt-5-nano".to_string());
    let env = make_live_env().await?;
    let agent_key = "opencode-multi-bot".to_string();
    seed_agent(
        &env.store,
        &agent_key,
        "OpenCode Multi Bot",
        "opencode",
        &model,
    )?;
    let spec = make_spec("OpenCode Multi Bot", &model, &env);

    let driver = OpencodeDriver;
    let attach = driver.attach(agent_key.clone(), spec.clone()).await?;
    let events = attach.events.clone();
    let mut rx = events.subscribe();
    let mut bootstrap = attach.handle;

    let outcome = async {
        bootstrap.start(StartOpts::default(), None).await?;
        let s1 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("bootstrap SessionAttached")?;

        let secondary_attach = driver.new_session(agent_key.clone(), spec.clone()).await?;
        let mut secondary = secondary_attach.handle;
        secondary.start(StartOpts::default(), None).await?;
        let s2 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("secondary SessionAttached")?;

        assert_ne!(
            s1, s2,
            "opencode session/new must mint a distinct session id"
        );

        bootstrap.close().await?;

        assert_eq!(
            secondary.session_id(),
            Some(s2.as_str()),
            "secondary session_id must survive bootstrap close"
        );

        // Probe secondary liveness: spawn a tertiary — only works if the
        // underlying opencode child + ACP connection persisted.
        let tertiary_attach = driver.new_session(agent_key.clone(), spec.clone()).await?;
        let mut tertiary = tertiary_attach.handle;
        tertiary.start(StartOpts::default(), None).await?;
        let s3 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("tertiary SessionAttached after bootstrap close")?;
        assert_ne!(s3, s2);

        tertiary.close().await?;
        secondary.close().await?;

        Ok::<_, anyhow::Error>(())
    }
    .await;

    env.bridge_ct.cancel();
    outcome
}

/// Claude: attach + start with a **trivial init prompt** → `SessionAttached(s1)`.
///
/// Claude is special: `claude -p` in stream-json mode does not emit a
/// `system/init` frame (and therefore no session id) until stdin delivers at
/// least one message. We send `"ok"` to force the init emission, then close
/// the bootstrap before the LLM has time to do much work. The secondary
/// handle then also needs a trivial prompt to mint its own session id.
///
/// Unlike the other runtimes, each Claude handle owns its own child (the CLI
/// can't multiplex), so "shared process survival" is not meaningful here —
/// what we're proving is that the driver's fan-out and registry entry remain
/// usable after a bootstrap close when a secondary is still live.
#[tokio::test]
#[ignore = "requires claude binary + Anthropic auth (ANTHROPIC_API_KEY or claude login)"]
async fn claude_multi_session_bootstrap_close_preserves_secondary() -> anyhow::Result<()> {
    if !binary_on_path("claude") {
        eprintln!("SKIP: `claude` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".to_string());
    let env = make_live_env().await?;
    let agent_key = "claude-multi-bot".to_string();
    seed_agent(&env.store, &agent_key, "Claude Multi Bot", "claude", &model)?;
    let spec = make_spec("Claude Multi Bot", &model, &env);

    let driver = ClaudeDriver;
    let attach = driver.attach(agent_key.clone(), spec.clone()).await?;
    let events = attach.events.clone();
    let mut rx = events.subscribe();
    let mut bootstrap = attach.handle;

    let trivial_prompt = || PromptReq {
        text: "ok".to_string(),
        attachments: vec![],
    };

    let outcome = async {
        bootstrap
            .start(StartOpts::default(), Some(trivial_prompt()))
            .await?;
        let s1 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("bootstrap SessionAttached (requires init prompt for claude -p)")?;

        let secondary_attach = driver.new_session(agent_key.clone(), spec.clone()).await?;
        let mut secondary = secondary_attach.handle;
        secondary
            .start(StartOpts::default(), Some(trivial_prompt()))
            .await?;
        let s2 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("secondary SessionAttached")?;

        assert_ne!(
            s1, s2,
            "each claude -p spawn must emit a distinct sessionId"
        );

        bootstrap.close().await?;

        assert_eq!(
            secondary.session_id(),
            Some(s2.as_str()),
            "secondary session_id must survive bootstrap close"
        );

        // Probe: secondary's child is still live → its session_id getter is
        // still Some and we can close it cleanly.
        secondary.close().await?;

        // After all handles close, a fresh attach on a new key should
        // succeed (registry pruned).
        let fresh_key = "claude-multi-bot-fresh".to_string();
        seed_agent(
            &env.store,
            &fresh_key,
            "Claude Multi Bot Fresh",
            "claude",
            &model,
        )?;
        let fresh = driver.attach(fresh_key.clone(), spec.clone()).await?;
        let mut fresh_rx = fresh.events.subscribe();
        let mut fresh_handle = fresh.handle;
        fresh_handle
            .start(StartOpts::default(), Some(trivial_prompt()))
            .await?;
        let _ = await_session_attached(&mut fresh_rx, Duration::from_secs(30), &fresh_key)
            .await
            .context("fresh attach SessionAttached")?;
        fresh_handle.close().await?;

        Ok::<_, anyhow::Error>(())
    }
    .await;

    env.bridge_ct.cancel();
    outcome
}

/// Codex: attach + start with a trivial prompt (so the thread persists a
/// rollout) → capture s1 → wait for turn completion → close →
/// `resume_session(s1)` in a fresh attach and confirm the thread id
/// round-trips.
///
/// The init prompt is required here: real codex 0.121+ rejects
/// `thread/resume` for any thread that has never run a turn with
/// `-32600 "no rollout found for thread id <id>"`. A turnless-resume hang
/// used to deadlock this suite; see
/// `codex_multi_session_resume_turnless_thread_surfaces_error` for the
/// regression test that proves the error now surfaces. For this test we
/// actually want resume to succeed, so we spend a few tokens to establish
/// the rollout on disk.
#[tokio::test]
#[ignore = "requires codex binary + OpenAI auth (OPENAI_API_KEY or codex login)"]
async fn codex_multi_session_resume_preserves_thread_id() -> anyhow::Result<()> {
    if !binary_on_path("codex") {
        eprintln!("SKIP: `codex` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_CODEX_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    let env = make_live_env().await?;
    let agent_key = "codex-resume-bot".to_string();
    seed_agent(&env.store, &agent_key, "Codex Resume Bot", "codex", &model)?;
    let spec = make_spec("Codex Resume Bot", &model, &env);

    let driver = CodexDriver;

    let trivial_prompt = || PromptReq {
        text: "reply with just ok".to_string(),
        attachments: vec![],
    };

    let s1 = {
        let attach = driver.attach(agent_key.clone(), spec.clone()).await?;
        let mut rx = attach.events.subscribe();
        let mut handle = attach.handle;

        let result = async {
            handle
                .start(StartOpts::default(), Some(trivial_prompt()))
                .await?;
            let s1 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
                .await
                .context("initial SessionAttached")?;

            // Wait for the turn to complete so the rollout gets flushed to
            // disk — otherwise `thread/resume` on a fresh attach trips
            // "no rollout found for thread id".
            let completed_at =
                tokio::time::Instant::now() + Duration::from_secs(90);
            loop {
                let remaining = completed_at.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(anyhow!(
                        "timed out waiting for Completed on initial turn"
                    ));
                }
                match timeout(remaining, rx.recv()).await {
                    Ok(Some(DriverEvent::Completed { .. })) => break,
                    Ok(Some(DriverEvent::Failed { error, .. })) => {
                        return Err(anyhow!(
                            "turn failed while seeding rollout: {error:?}"
                        ));
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => {
                        return Err(anyhow!("event stream closed before Completed"));
                    }
                    Err(_) => {
                        return Err(anyhow!("timed out waiting for Completed on initial turn"));
                    }
                }
            }

            handle.close().await?;
            Ok::<_, anyhow::Error>(s1)
        }
        .await;

        result?
    };

    // Round trip: attach fresh + resume_session with s1.
    let attach2 = driver.attach(agent_key.clone(), spec.clone()).await?;
    let mut rx2 = attach2.events.subscribe();
    let _bootstrap2 = attach2.handle; // keep alive; not starting it

    let resumed = driver
        .resume_session(agent_key.clone(), spec.clone(), s1.clone())
        .await?;
    let mut resumed_handle = resumed.handle;

    let outcome = async {
        resumed_handle.start(StartOpts::default(), None).await?;
        let s1_again = await_session_attached(&mut rx2, Duration::from_secs(30), &agent_key)
            .await
            .context("resumed SessionAttached")?;
        assert_eq!(
            s1_again, s1,
            "codex thread/resume must report the same thread id"
        );
        assert_eq!(
            resumed_handle.session_id(),
            Some(s1.as_str()),
            "resumed handle session_id matches"
        );
        resumed_handle.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;

    env.bridge_ct.cancel();
    outcome
}

/// Codex: attach + start (mints s1 with no turn ever run) → `resume_session(s1)`
/// must surface the app-server's `-32600 "no rollout found for thread id"`
/// error as a real `Err` rather than hanging forever.
///
/// This is the regression test for the wake-on-error fix in the Codex driver.
/// Prior to the fix, `parse_response_by_method` short-circuited on the error
/// branch before consulting the request registry, so the reader loop never
/// removed the pending `thread/resume` entry and its `oneshot::Sender` stayed
/// parked. `start_or_resume_thread`'s `rx.await` would block indefinitely and
/// the whole `start()` call would hang. The fix makes the parser always call
/// `method_for_id(id)` so the reader-loop closure removes the entry and fires
/// the waker with `AppServerEvent::Error`, which `start_or_resume_thread`
/// already translates into a typed `bail!`.
///
/// Why this shape: real codex 0.121+ rejects `thread/resume` on any thread
/// that hasn't persisted a rollout (i.e. hasn't run a turn). A pristine
/// `thread/start` followed by immediate `thread/resume` of that id is the
/// cheapest way to trigger the error without spending tokens.
#[tokio::test]
#[ignore = "requires codex binary + OpenAI auth (OPENAI_API_KEY or codex login)"]
async fn codex_multi_session_resume_turnless_thread_surfaces_error() -> anyhow::Result<()> {
    if !binary_on_path("codex") {
        eprintln!("SKIP: `codex` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_CODEX_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    let env = make_live_env().await?;
    let agent_key = "codex-turnless-resume-bot".to_string();
    seed_agent(
        &env.store,
        &agent_key,
        "Codex Turnless Resume Bot",
        "codex",
        &model,
    )?;
    let spec = make_spec("Codex Turnless Resume Bot", &model, &env);

    let driver = CodexDriver;

    // Mint s1 via bootstrap attach + start (no prompt → no turn → no rollout).
    let attach = driver.attach(agent_key.clone(), spec.clone()).await?;
    let mut rx = attach.events.subscribe();
    let mut bootstrap = attach.handle;

    let outcome = async {
        bootstrap.start(StartOpts::default(), None).await?;
        let s1 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
            .await
            .context("bootstrap SessionAttached")?;

        // Probe: attempt to resume s1. Against real codex this hits the
        // "no rollout found for thread id" path and the driver MUST surface
        // that error rather than hang. Bound the call in a 30 s timeout so a
        // regression (driver hang) fails the test instead of the suite.
        let resume_attach = driver
            .resume_session(agent_key.clone(), spec.clone(), s1.clone())
            .await?;
        let mut resume_handle = resume_attach.handle;

        let start_result = timeout(
            Duration::from_secs(30),
            resume_handle.start(StartOpts::default(), None),
        )
        .await
        .context(
            "resume_session start() timed out — driver still hangs on thread/resume error response",
        )?;

        let err = start_result.expect_err(
            "turnless thread/resume must surface an error; driver accepted it or returned Ok",
        );
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("no rollout") || msg.contains("thread id") || msg.contains("rejected"),
            "unexpected error shape from turnless resume: {msg}"
        );

        // Cleanup. `close()` on a handle that failed to start must still be
        // safe to call.
        let _ = resume_handle.close().await;
        bootstrap.close().await?;

        Ok::<_, anyhow::Error>(())
    }
    .await;

    env.bridge_ct.cancel();
    outcome
}

/// Claude: attach + start (with trivial prompt to force init) → capture s1 →
/// close → `resume_session(s1)` in a fresh attach → confirm s1 round-trips.
///
/// Unlike Codex, Claude persists sessions on disk, so resume works across a
/// clean close+reattach.
#[tokio::test]
#[ignore = "requires claude binary + Anthropic auth (ANTHROPIC_API_KEY or claude login)"]
async fn claude_multi_session_resume_preserves_session_id() -> anyhow::Result<()> {
    if !binary_on_path("claude") {
        eprintln!("SKIP: `claude` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".to_string());
    let env = make_live_env().await?;
    let agent_key = "claude-resume-bot".to_string();
    seed_agent(&env.store, &agent_key, "Claude Resume Bot", "claude", &model)?;
    let spec = make_spec("Claude Resume Bot", &model, &env);

    let driver = ClaudeDriver;
    let trivial_prompt = || PromptReq {
        text: "ok".to_string(),
        attachments: vec![],
    };

    let s1 = {
        let attach = driver.attach(agent_key.clone(), spec.clone()).await?;
        let mut rx = attach.events.subscribe();
        let mut handle = attach.handle;

        let result = async {
            handle
                .start(StartOpts::default(), Some(trivial_prompt()))
                .await?;
            let s1 = await_session_attached(&mut rx, Duration::from_secs(30), &agent_key)
                .await
                .context("initial SessionAttached (claude requires init prompt)")?;
            // Give the child a moment to flush any queued session state to
            // disk before we SIGTERM it.
            tokio::time::sleep(Duration::from_millis(200)).await;
            handle.close().await?;
            Ok::<_, anyhow::Error>(s1)
        }
        .await;

        result?
    };

    // Round trip: attach fresh + resume_session with s1.
    let attach2 = driver.attach(agent_key.clone(), spec.clone()).await?;
    let mut rx2 = attach2.events.subscribe();
    let _bootstrap2 = attach2.handle;

    let resumed = driver
        .resume_session(agent_key.clone(), spec.clone(), s1.clone())
        .await?;
    let mut resumed_handle = resumed.handle;

    let outcome = async {
        // Resume still needs stdin input to emit init; send a trivial prompt.
        resumed_handle
            .start(StartOpts::default(), Some(trivial_prompt()))
            .await?;
        let s1_again = await_session_attached(&mut rx2, Duration::from_secs(30), &agent_key)
            .await
            .context("resumed SessionAttached")?;
        assert_eq!(
            s1_again, s1,
            "claude --resume <id> must report the same sessionId"
        );
        assert_eq!(
            resumed_handle.session_id(),
            Some(s1.as_str()),
            "resumed handle session_id matches"
        );
        resumed_handle.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;

    env.bridge_ct.cancel();
    outcome
}

// ---------------------------------------------------------------------------
// Helper sanity tests (only these run under plain `cargo test`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod harness_unit_tests {
    use super::*;

    #[test]
    fn debug_event_kind_formats_variants() {
        let e = DriverEvent::SessionAttached {
            key: "k".into(),
            session_id: "sid-1".into(),
        };
        assert_eq!(debug_event_kind(&e), "SessionAttached(sid-1)");
    }

    #[tokio::test]
    async fn await_session_attached_returns_session_id_on_match() {
        let (events, tx) = chorus::agent::drivers::EventFanOut::new();
        let mut rx = events.subscribe();
        tx.send(DriverEvent::Lifecycle {
            key: "k".into(),
            state: chorus::agent::drivers::AgentState::Starting,
        })
        .await
        .unwrap();
        tx.send(DriverEvent::SessionAttached {
            key: "k".into(),
            session_id: "sid-42".into(),
        })
        .await
        .unwrap();

        let got = await_session_attached(&mut rx, Duration::from_secs(2), &"k".to_string())
            .await
            .unwrap();
        assert_eq!(got, "sid-42");
    }

    #[tokio::test]
    async fn await_session_attached_times_out_with_descriptive_error() {
        let (events, _tx) = chorus::agent::drivers::EventFanOut::new();
        let mut rx = events.subscribe();
        let err = await_session_attached(&mut rx, Duration::from_millis(100), &"k".to_string())
            .await
            .expect_err("should time out");
        let msg = format!("{err:#}");
        assert!(msg.contains("timed out"), "got: {msg}");
    }
}

