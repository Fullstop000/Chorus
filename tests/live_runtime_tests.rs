//! Live runtime integration tests for the shared MCP bridge.
//!
//! These tests exercise the full Phase 1+2 stack end-to-end against a **real**
//! runtime binary (not a mock or in-process MCP client). They prove that:
//!
//!   1. A driver, given `AgentSpec.bridge_endpoint = Some(url)`, pairs with
//!      the shared bridge via `/admin/pair` and wires its config to
//!      `{bridge_url}/token/{token}/mcp`.
//!   2. The runtime process connects to the bridge over native HTTP MCP.
//!   3. When the runtime calls the `send_message` tool, the bridge routes it
//!      through `ChatBridge` → `ChorusBackend` → Chorus server → SQLite.
//!   4. The resulting message is observable via the Chorus store's history.
//!
//! # Running
//!
//! All tests are `#[ignore]` by default — `cargo test` will skip them. Run
//! them explicitly:
//!
//! ```bash
//! cargo test --test live_runtime_tests -- --ignored --nocapture
//! ```
//!
//! To run a single runtime's test:
//!
//! ```bash
//! cargo test --test live_runtime_tests claude_agent_replies -- --ignored --nocapture
//! cargo test --test live_runtime_tests codex_agent_replies -- --ignored --nocapture
//! cargo test --test live_runtime_tests gemini_agent_replies -- --ignored --nocapture
//! cargo test --test live_runtime_tests kimi_agent_replies  -- --ignored --nocapture
//! ```
//!
//! Tests take 10–60 seconds each due to real runtime latency. If the required
//! binary is missing on `PATH`, the test prints a skip message and returns
//! `Ok(())` rather than failing.
//!
//! # Required environment
//!
//! | Test                                          | Binary    | Auth / env vars                                                                   | Model env var               |
//! |-----------------------------------------------|-----------|-----------------------------------------------------------------------------------|-----------------------------|
//! | `opencode_agent_replies_through_shared_bridge`| `opencode`| `opencode auth login` (OAuth); no API key needed                                  | `OPENCODE_MODEL` (optional, default `opencode/gpt-5-nano`) |
//! | `claude_agent_replies_through_shared_bridge`  | `claude`  | `ANTHROPIC_API_KEY` env var **or** OAuth via `claude login`                       | `CHORUS_TEST_CLAUDE_MODEL`  (optional, default `sonnet`) |
//! | `codex_agent_replies_through_shared_bridge`   | `codex`   | `OPENAI_API_KEY` env var **or** `codex login`; note: HTTP MCP may be unstable in some Codex versions (see RUNTIME_MCP_SUPPORT.md) | `CHORUS_TEST_CODEX_MODEL` (optional, default `gpt-5.4`) |
//! | `gemini_agent_replies_through_shared_bridge`  | `gemini`  | OAuth via `gemini auth login` stored in `~/.gemini/oauth_creds.json` or `GEMINI_API_KEY` | `CHORUS_TEST_GEMINI_MODEL` (optional, default `gemini-2.5-flash`) |
//! | `kimi_agent_replies_through_shared_bridge`    | `kimi`    | Moonshot credentials in `~/.kimi/credentials/kimi-code.json`                     | `CHORUS_TEST_KIMI_MODEL` (optional, default `kimi-code/kimi-for-coding`) |
//!
//! # Debugging
//!
//! Use `--nocapture` to see runtime stderr piped through our tracing setup.
//! If a test hangs, check whether the runtime has valid auth in its config dir.
//!
//! # Coverage matrix
//!
//! The shared-bridge runtime path is covered by four test layers:
//!
//! | Layer | Test location | What it proves |
//! |-------|--------------|----------------|
//! | Bridge HTTP layer | `tests/bridge_serve_tests.rs` | In-process bridge starts, health, sessions, `send_message` → store |
//! | Discovery file I/O | `src/bridge/discovery.rs` (unit tests) | `write_bridge_info_to` / `read_bridge_info_from` roundtrip, stale PID, corrupt file, live-PID stomp guard |
//! | `resolve_bridge_endpoint` | `src/agent/manager.rs` (`resolve_bridge_endpoint_returns_override_when_set`, `resolve_bridge_endpoint_fails_loudly_without_bridge`) | Override path Ok, no-bridge path Err with user-visible message |
//! | Driver + bridge round-trip | This file (4 `#[ignore]` live tests) | Real runtime binary wired to `bridge_endpoint: String` → message lands in store |
//!
//! The one composition that is not tested by automation is the
//! `AgentManager::start_agent` → `read_bridge_info()` path when a real
//! discovery file is present on the developer's machine. That path would
//! require writing to `~/.chorus/bridge.json` (global, unsafe in CI); the
//! combination of discovery unit tests + `resolve_bridge_endpoint` override
//! tests covers everything that doesn't require a real bridge process.

mod harness;
use harness::join_channel_silent;
use std::sync::Arc;
use std::time::Duration;

use chorus::agent::drivers::claude::ClaudeDriver;
use chorus::agent::drivers::codex::CodexDriver;
use chorus::agent::drivers::gemini::GeminiDriver;
use chorus::agent::drivers::kimi::KimiDriver;
use chorus::agent::drivers::opencode::OpencodeDriver;
use chorus::agent::drivers::{AgentSpec, PromptReq, RuntimeDriver, SessionIntent};

use chorus::agent::AgentLifecycle;
use chorus::bridge::serve::build_bridge_router;

use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, SenderType};
use chorus::store::{AgentRecordUpsert, Store};

// ---------------------------------------------------------------------------
// Helpers (duplicated from bridge_serve_tests.rs; cargo integration tests
// don't share modules across files without extra wiring — keeping each test
// file self-contained is the existing convention).
// ---------------------------------------------------------------------------

/// No-op lifecycle used when running the Chorus server in-process for tests.
struct NoopLifecycle;

impl AgentLifecycle for NoopLifecycle {
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

    fn process_state<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Option<chorus::agent::drivers::ProcessState>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async { None })
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        vec![]
    }
}

/// Start a Chorus server in-process with an in-memory SQLite store. Returns
/// the server's base URL and the shared `Store`.
async fn start_chorus_server() -> anyhow::Result<(String, Arc<Store>, String)> {
    let store = Arc::new(Store::open(":memory:")?);
    let tester = store.create_local_human("tester")?;
    store.create_channel("general", Some("General"), ChannelType::Channel, None)?;
    join_channel_silent(&store, "general", &tester.id, "human");

    let router = harness::build_router_with_lifecycle(store.clone(), Arc::new(NoopLifecycle));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok((url, store, tester.id))
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
///
/// Intentionally simple: we don't want a `which` dev-dep just for this one
/// check, and `command_exists` in `src/agent/drivers/mod.rs` is `pub(crate)`.
fn binary_on_path(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Return the path of the newest `.log` file in `dir`, or `None` if the
/// directory does not exist or contains no log files.
fn newest_log_in(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().map(|t| t.is_file()).unwrap_or(false)
                && e.path().extension().is_some_and(|x| x == "log")
        })
        .max_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
        .map(|e| e.path())
}

/// Return the well-known log path for a given runtime, or `None` if the path
/// does not exist on this machine.
fn runtime_log_path(runtime_name: &str) -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    match runtime_name {
        "claude" => {
            // Claude Code log directory — newest file.
            let log_dir = home.join(".claude").join("logs");
            newest_log_in(&log_dir)
        }
        "codex" => {
            // Codex TUI log (may or may not apply to app-server mode).
            let path = home.join(".codex").join("log").join("codex-tui.log");
            if path.exists() {
                Some(path)
            } else {
                None
            }
        }
        "kimi" => {
            // Kimi writes to ~/.kimi/logs/kimi.log — the most common signal source.
            let path = home.join(".kimi").join("logs").join("kimi.log");
            if path.exists() {
                Some(path)
            } else {
                None
            }
        }
        "opencode" => {
            let log_dir = home.join(".opencode").join("logs");
            newest_log_in(&log_dir)
        }
        _ => None,
    }
}

/// Hard cap on how many trailing bytes of a runtime log we slurp into memory
/// for the diagnostic dump. Set to 256 KiB — enough to hold well over 200
/// lines of typical runtime log output without ever reading a multi-GB log.
const DIAGNOSTIC_LOG_TAIL_BYTES: u64 = 256 * 1024;

/// How many trailing lines of that tail we actually print.
const DIAGNOSTIC_LOG_TAIL_LINES: usize = 200;

/// Redact `/token/{token}/mcp` URL segments so pairing bearer tokens don't
/// leak into CI-archived test logs. Replaces the `{token}` with `[REDACTED]`
/// while leaving the surrounding URL visible so operators can still see the
/// port / path shape when debugging. Non-URL occurrences of `token` are left
/// alone.
fn redact_pairing_tokens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(idx) = rest.find("/token/") {
        out.push_str(&rest[..idx]);
        out.push_str("/token/[REDACTED]");
        rest = &rest[idx + "/token/".len()..];
        // Skip past the token itself up to the next `/` or line terminator.
        let end = rest.find(['/', '\n', '"']).unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Read the trailing `max_bytes` of a file without loading the whole thing.
/// Returns `io::Error` if the file can't be opened or read.
/// If a seek was performed (file larger than cap), trims up to the first
/// newline so the output starts on a clean line. Files smaller than the cap
/// are returned in full — trimming them would silently drop the first line.
/// Any invalid UTF-8 bytes are replaced via lossy conversion.
fn read_log_tail(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let to_read = std::cmp::min(len, max_bytes);
    let seeked = to_read < len;
    if seeked {
        f.seek(SeekFrom::End(-(to_read as i64)))?;
    }
    let mut buf = Vec::with_capacity(to_read as usize);
    f.take(to_read).read_to_end(&mut buf)?;
    let start = if seeked {
        // Seek likely landed mid-line; trim up to the first newline.
        buf.iter()
            .position(|&b| b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0)
    } else {
        0
    };
    Ok(String::from_utf8_lossy(&buf[start..]).into_owned())
}

/// Dump all available diagnostic signals before an `Err` is returned from a
/// live runtime test.  Returns a `String` suitable for appending to the
/// anyhow error message.
///
/// Collects (best-effort, never panics):
/// 1. Last `DIAGNOSTIC_LOG_TAIL_LINES` lines (≤ `DIAGNOSTIC_LOG_TAIL_BYTES`)
///    of the runtime's log file (if path provided and readable)
/// 2. Contents of any MCP config files the driver wrote in `working_dir`
/// 3. Observable state (channel history, captured by caller)
/// 4. Hint for surfacing runtime stderr via RUST_LOG on re-run
///
/// Note on runtime stderr: the drivers spawn async tasks that consume stderr
/// and re-emit it via `tracing::warn!`.  Re-capturing it here would require
/// invasive driver changes.  The RUST_LOG hint below is the lowest-friction
/// workaround — it surfaces the stderr on the next explicit re-run.
fn collect_failure_diagnostics(
    runtime_name: &str,
    runtime_log_path: Option<&std::path::Path>,
    working_dir: &std::path::Path,
    channel_history: &str,
) -> String {
    let mut out = String::new();
    out.push_str("\n\n=== FAILURE DIAGNOSTICS ===\n");

    // 1. Runtime log file — tail only; large logs would otherwise blow memory.
    out.push_str(&format!("\n--- {} log file ---\n", runtime_name));
    if let Some(log_path) = runtime_log_path {
        out.push_str(&format!("Path: {}\n", log_path.display()));
        match read_log_tail(log_path, DIAGNOSTIC_LOG_TAIL_BYTES) {
            Ok(tail) => {
                let lines: Vec<&str> = tail.lines().collect();
                let start = lines.len().saturating_sub(DIAGNOSTIC_LOG_TAIL_LINES);
                for line in &lines[start..] {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Err(e) => {
                out.push_str(&format!("(not readable: {})\n", e));
            }
        }
    } else {
        out.push_str("(no log path documented for this runtime)\n");
    }

    // 2. MCP config files in working_dir.
    out.push_str(&format!(
        "\n--- MCP config files in {} ---\n",
        working_dir.display()
    ));
    if let Ok(entries) = std::fs::read_dir(working_dir) {
        let mut found = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Only dump files the drivers actually write — matching every
            // `.json` would sweep in unrelated config and risk leaking
            // user secrets that happen to live in working_dir.
            //   - `.chorus-claude-mcp.json`  (ClaudeDriver)
            //   - `.chorus-kimi-mcp.json`    (KimiDriver)
            //   - `opencode.json`            (OpencodeDriver)
            //   - Codex configures via `-c` flags, no file on disk.
            let is_driver_mcp_config =
                name_str.contains("-mcp.json") || name_str == "opencode.json";
            if is_driver_mcp_config {
                found = true;
                out.push_str(&format!("\n  {}:\n", entry.path().display()));
                match std::fs::read_to_string(entry.path()) {
                    Ok(contents) => {
                        // Redact the pairing token in MCP URLs. These are
                        // bearer credentials — even with a 5-min TTL, dumping
                        // them into CI-archived logs is sloppy.
                        let redacted = redact_pairing_tokens(&contents);
                        for line in redacted.lines() {
                            out.push_str("    ");
                            out.push_str(line);
                            out.push('\n');
                        }
                    }
                    Err(e) => {
                        out.push_str(&format!("    (not readable: {})\n", e));
                    }
                }
            }
        }
        if !found {
            out.push_str("(no MCP config files found)\n");
        }
    } else {
        out.push_str("(working dir not readable)\n");
    }

    // 3. Observable channel state.
    out.push_str("\n--- Channel state at failure ---\n");
    out.push_str(channel_history);

    // 4. Hint for surfacing runtime stderr on re-run.
    out.push_str("\n\n--- Runtime stderr ---\n");
    out.push_str(
        "Not captured directly by this helper (captured by driver's internal tokio task \
        and emitted via tracing::warn!). To see runtime stderr, re-run with:\n",
    );
    out.push_str(&format!(
        "  RUST_LOG=chorus::agent::drivers::{}=debug \
        cargo test --test live_runtime_tests {}_agent_replies_through_shared_bridge \
        -- --ignored --nocapture\n",
        runtime_name, runtime_name
    ));

    out.push_str("\n=== END DIAGNOSTICS ===\n");
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Live round-trip: spawn a real `opencode` runtime, send it a prompt, verify
/// its reply lands in the Chorus store via the shared bridge.
///
/// Requires:
///   - `opencode` binary on PATH (tested with 1.3.13)
///   - `opencode` must already be logged in (`opencode auth login`)
///   - Optional: `OPENCODE_MODEL` env var (defaults to `opencode/gpt-5-nano`)
///
/// What it proves:
///   - `OpencodeDriver::attach` + `AgentHandle::start` with
///     `bridge_endpoint = Some(bridge_url)` configures the runtime's MCP
///     client to use the token URL.
///   - The runtime's `send_message` tool call routes through the shared
///     bridge into the Chorus store under the agent's identity.
#[tokio::test]
#[ignore = "requires opencode binary + login; run with --ignored"]
async fn opencode_agent_replies_through_shared_bridge() -> anyhow::Result<()> {
    // 1. Skip if opencode binary not on PATH.
    if !binary_on_path("opencode") {
        eprintln!("SKIP: `opencode` binary not found on PATH");
        return Ok(());
    }

    let model =
        std::env::var("OPENCODE_MODEL").unwrap_or_else(|_| "opencode/gpt-5-nano".to_string());

    // 2. Start Chorus server + shared bridge.
    let (server_url, store, tester_id) = start_chorus_server().await?;
    let (bridge_url, bridge_ct) = start_bridge_with_server(&server_url).await?;

    // 3. Seed agent record and channel membership. The agent's `sender_name`
    //    must match the key we pass to the driver so messages posted via the
    //    bridge are attributed to it.
    let agent_key = "opencode-live-bot";
    let agent_id = store.create_agent_record(&AgentRecordUpsert {
        name: agent_key,
        display_name: "OpenCode Live Bot",
        description: None,
        system_prompt: None,
        runtime: "opencode",
        model: &model,
        reasoning_effort: None,
        machine_id: None,
        env_vars: &[],
    })?;
    join_channel_silent(&store, "general", &agent_id, "agent");

    // 4. Seed a user message in #general so the agent has context to reply to.
    //    The prompt we send directly via the handle instructs the runtime
    //    what to do; this message exists mostly to give the conversation
    //    shape and to have a thread for replies to land in.
    store.create_message(CreateMessage {
        channel_name: "general",
        sender_id: &tester_id,
        sender_type: SenderType::Human,
        content: "@opencode-live-bot please reply to everyone",
        attachment_ids: &[],
        suppress_event: false,
        run_id: None,
    })?;

    // 5. Build AgentSpec pointing at the shared bridge — the code path we're
    //    validating.
    let tmpdir = tempfile::tempdir()?;
    let spec = AgentSpec {
        display_name: "OpenCode Live Bot".to_string(),
        description: None,
        system_prompt: None,
        model: model.clone(),
        reasoning_effort: None,
        env_vars: vec![],
        working_directory: tmpdir.path().to_path_buf(),
        bridge_endpoint: bridge_url.clone(),
    };

    // 6. Open a new session and run the runtime, deferring the first prompt so
    //    it's delivered once the ACP session is active.
    let driver = OpencodeDriver;
    let attach_result = driver
        .open_session(agent_key.to_string(), spec, SessionIntent::New)
        .await?;
    let mut handle = attach_result.session;

    let prompt = PromptReq {
        text: format!(
            "You are an agent named `{agent_key}` in a chat channel. \
             Use the `send_message` tool to post the exact text \
             `hello world` to the channel `#general`. Do not include any \
             other commentary — just call the tool once and stop."
        ),
        attachments: vec![],
    };

    handle.run(Some(prompt)).await?;

    // 7. Poll the store for up to 60 seconds waiting for the agent's reply.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        if messages
            .iter()
            .any(|m| m.sender_name == agent_key && m.content.to_lowercase().contains("hello world"))
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // 8. Clean up the runtime process regardless of outcome.
    let _ = handle.close().await;
    bridge_ct.cancel();

    if !found {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        let history_str = format!(
            "{:#?}",
            messages
                .iter()
                .map(|m| (&m.sender_name, &m.content))
                .collect::<Vec<_>>()
        );
        let diagnostics = collect_failure_diagnostics(
            "opencode",
            runtime_log_path("opencode").as_deref(),
            tmpdir.path(),
            &history_str,
        );
        anyhow::bail!(
            "agent did not send a reply containing 'hello world' within 60s{}",
            diagnostics
        );
    }

    Ok(())
}

/// Live round-trip: spawn a real `claude` (Claude Code CLI) runtime, send it a
/// prompt, verify its reply lands in the Chorus store via the shared bridge.
///
/// Requires:
///   - `claude` binary on PATH
///   - Auth: either `ANTHROPIC_API_KEY` env var set, or already logged in via
///     `claude login`. The runtime handles auth itself; we do NOT skip if the
///     key is absent — a missing key produces a clear auth error from Claude.
///   - Optional: `CHORUS_TEST_CLAUDE_MODEL` env var (defaults to `sonnet`)
///
/// What it proves:
///   - `ClaudeDriver::attach` + `AgentHandle::start` with
///     `bridge_endpoint = Some(bridge_url)` writes a `.chorus-claude-mcp.json`
///     config with `"type": "http"` and the token URL, then spawns `claude -p`
///     in stream-json mode wired to the shared bridge.
///   - The runtime's `send_message` MCP tool call routes through the bridge
///     into the Chorus store under the agent's identity.
#[tokio::test]
#[ignore = "requires claude binary + Anthropic auth (ANTHROPIC_API_KEY or claude login)"]
async fn claude_agent_replies_through_shared_bridge() -> anyhow::Result<()> {
    // 1. Skip if claude binary not on PATH.
    if !binary_on_path("claude") {
        eprintln!("SKIP: `claude` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".to_string());

    // 2. Start Chorus server + shared bridge.
    let (server_url, store, tester_id) = start_chorus_server().await?;
    let (bridge_url, bridge_ct) = start_bridge_with_server(&server_url).await?;

    // 3. Seed agent record and channel membership.
    let agent_key = "claude-live-bot";
    let agent_id = store.create_agent_record(&AgentRecordUpsert {
        name: agent_key,
        display_name: "Claude Live Bot",
        description: None,
        system_prompt: None,
        runtime: "claude",
        model: &model,
        reasoning_effort: None,
        machine_id: None,
        env_vars: &[],
    })?;
    join_channel_silent(&store, "general", &agent_id, "agent");

    // 4. Seed a user message in #general for conversation shape.
    store.create_message(CreateMessage {
        channel_name: "general",
        sender_id: &tester_id,
        sender_type: SenderType::Human,
        content: "@claude-live-bot please reply to everyone",
        attachment_ids: &[],
        suppress_event: false,
        run_id: None,
    })?;

    // 5. Build AgentSpec with bridge_endpoint set.
    let tmpdir = tempfile::tempdir()?;
    let spec = AgentSpec {
        display_name: "Claude Live Bot".to_string(),
        description: None,
        system_prompt: None,
        model: model.clone(),
        reasoning_effort: None,
        env_vars: vec![],
        working_directory: tmpdir.path().to_path_buf(),
        bridge_endpoint: bridge_url.clone(),
    };

    // 6. Open a new session and run the runtime with initial prompt.
    let driver = ClaudeDriver;
    let attach_result = driver
        .open_session(agent_key.to_string(), spec, SessionIntent::New)
        .await?;
    let mut handle = attach_result.session;

    let prompt = PromptReq {
        text: format!(
            "You are an agent named `{agent_key}` in a chat channel. \
             Use the `send_message` tool to post the exact text \
             `hello world` to the channel `#general`. Do not include any \
             other commentary — just call the tool once and stop."
        ),
        attachments: vec![],
    };

    handle.run(Some(prompt)).await?;

    // 7. Poll the store for up to 60 seconds waiting for the agent's reply.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        if messages
            .iter()
            .any(|m| m.sender_name == agent_key && m.content.to_lowercase().contains("hello world"))
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // 8. Clean up.
    let _ = handle.close().await;
    bridge_ct.cancel();

    if !found {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        let history_str = format!(
            "{:#?}",
            messages
                .iter()
                .map(|m| (&m.sender_name, &m.content))
                .collect::<Vec<_>>()
        );
        let diagnostics = collect_failure_diagnostics(
            "claude",
            runtime_log_path("claude").as_deref(),
            tmpdir.path(),
            &history_str,
        );
        anyhow::bail!(
            "agent did not send a reply containing 'hello world' within 60s{}",
            diagnostics
        );
    }

    Ok(())
}

/// Live round-trip: spawn a real `codex` (OpenAI Codex CLI) runtime, send it a
/// prompt, verify its reply lands in the Chorus store via the shared bridge.
///
/// Requires:
///   - `codex` binary on PATH (uses `codex app-server` native protocol)
///   - `OPENAI_API_KEY` env var set, or already logged in via `codex login`
///   - Optional: `CHORUS_TEST_CODEX_MODEL` env var (defaults to `gpt-5.4`)
///
/// **Note on HTTP MCP stability**: Several GitHub issues (openai/codex #4707,
/// #5208, #11284) report instability with streamable HTTP in certain Codex
/// versions. If this test fails with MCP transport errors rather than auth or
/// logic errors, that is consistent with the known issue documented in
/// `docs/RUNTIME_MCP_SUPPORT.md`. The test remains in place for verification
/// once a stable Codex version is available.
///
/// What it proves:
///   - `CodexDriver::attach` + `AgentHandle::start` with
///     `bridge_endpoint = Some(bridge_url)` passes `-c mcp_servers.chat.url=…`
///     flags to `codex app-server`, wiring it to the shared bridge.
///   - The runtime's `send_message` MCP tool call routes through the bridge
///     into the Chorus store under the agent's identity.
#[tokio::test]
#[ignore = "requires codex binary + OpenAI auth (OPENAI_API_KEY or codex login)"]
async fn codex_agent_replies_through_shared_bridge() -> anyhow::Result<()> {
    // 1. Skip if codex binary not on PATH.
    if !binary_on_path("codex") {
        eprintln!("SKIP: `codex` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_CODEX_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());

    // 2. Start Chorus server + shared bridge.
    let (server_url, store, tester_id) = start_chorus_server().await?;
    let (bridge_url, bridge_ct) = start_bridge_with_server(&server_url).await?;

    // 3. Seed agent record and channel membership.
    let agent_key = "codex-live-bot";
    let agent_id = store.create_agent_record(&AgentRecordUpsert {
        name: agent_key,
        display_name: "Codex Live Bot",
        description: None,
        system_prompt: None,
        runtime: "codex",
        model: &model,
        reasoning_effort: None,
        machine_id: None,
        env_vars: &[],
    })?;
    join_channel_silent(&store, "general", &agent_id, "agent");

    // 4. Seed a user message in #general for conversation shape.
    store.create_message(CreateMessage {
        channel_name: "general",
        sender_id: &tester_id,
        sender_type: SenderType::Human,
        content: "@codex-live-bot please reply to everyone",
        attachment_ids: &[],
        suppress_event: false,
        run_id: None,
    })?;

    // 5. Build AgentSpec with bridge_endpoint set.
    let tmpdir = tempfile::tempdir()?;
    let spec = AgentSpec {
        display_name: "Codex Live Bot".to_string(),
        description: None,
        system_prompt: None,
        model: model.clone(),
        reasoning_effort: None,
        env_vars: vec![],
        working_directory: tmpdir.path().to_path_buf(),
        bridge_endpoint: bridge_url.clone(),
    };

    // 6. Open a new session and run the runtime with initial prompt.
    //    Codex uses the `app-server` native protocol; the driver passes
    //    `-c mcp_servers.chat.url=…` flags to wire up the HTTP bridge.
    //    (Codex app-server doesn't write a log file — its only signal is
    //    stdout JSON-RPC. The Iron Rule helper will note this on failure.)
    let driver = CodexDriver;
    let attach_result = driver
        .open_session(agent_key.to_string(), spec, SessionIntent::New)
        .await?;
    let mut handle = attach_result.session;

    let prompt = PromptReq {
        text: format!(
            "You are an agent named `{agent_key}` in a chat channel. \
             Use the `send_message` tool to post the exact text \
             `hello world` to the channel `#general`. Do not include any \
             other commentary — just call the tool once and stop."
        ),
        attachments: vec![],
    };

    handle.run(Some(prompt)).await?;

    // 7. Poll the store for up to 120s waiting for the agent's reply.
    //    Codex on gpt-5.4 via WebSocket + MCP tool round-trip can exceed 60s
    //    on a cold cache — extending keeps this test reliable.
    let codex_deadline_secs = 120u64;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(codex_deadline_secs);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        if messages
            .iter()
            .any(|m| m.sender_name == agent_key && m.content.to_lowercase().contains("hello world"))
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // 8. Clean up.
    let _ = handle.close().await;
    bridge_ct.cancel();

    if !found {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        let history_str = format!(
            "{:#?}",
            messages
                .iter()
                .map(|m| (&m.sender_name, &m.content))
                .collect::<Vec<_>>()
        );
        let diagnostics = collect_failure_diagnostics(
            "codex",
            runtime_log_path("codex").as_deref(),
            tmpdir.path(),
            &history_str,
        );
        anyhow::bail!(
            "agent did not send a reply containing 'hello world' within {}s{}",
            codex_deadline_secs,
            diagnostics
        );
    }

    Ok(())
}

/// Live round-trip: spawn a real `gemini` runtime, send it a prompt, verify
/// its reply lands in the Chorus store via the shared bridge.
///
/// Requires:
///   - `gemini` binary on PATH
///   - OAuth credentials in `~/.gemini/oauth_creds.json` via `gemini auth login`
///     or `GEMINI_API_KEY` in the environment
///   - Optional: `CHORUS_TEST_GEMINI_MODEL` env var (defaults to
///     `gemini-2.5-flash`)
///
/// What it proves:
///   - `GeminiDriver::open_session` + `run` with `bridge_endpoint = bridge_url`
///     spawns `gemini --acp` successfully and wires the runtime to the shared
///     HTTP MCP bridge.
///   - The runtime's `send_message` MCP tool call routes through the bridge
///     into the Chorus store under the agent's identity.
#[tokio::test]
#[ignore = "requires gemini binary + auth (~/.gemini/oauth_creds.json or GEMINI_API_KEY)"]
async fn gemini_agent_replies_through_shared_bridge() -> anyhow::Result<()> {
    if !binary_on_path("gemini") {
        eprintln!("SKIP: `gemini` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_GEMINI_MODEL")
        .unwrap_or_else(|_| "gemini-2.5-flash".to_string());

    let (server_url, store, tester_id) = start_chorus_server().await?;
    let (bridge_url, bridge_ct) = start_bridge_with_server(&server_url).await?;

    let agent_key = "gemini-live-bot";
    let agent_id = store.create_agent_record(&AgentRecordUpsert {
        name: agent_key,
        display_name: "Gemini Live Bot",
        description: None,
        system_prompt: None,
        runtime: "gemini",
        model: &model,
        reasoning_effort: None,
        machine_id: None,
        env_vars: &[],
    })?;
    join_channel_silent(&store, "general", &agent_id, "agent");

    store.create_message(CreateMessage {
        channel_name: "general",
        sender_id: &tester_id,
        sender_type: SenderType::Human,
        content: "@gemini-live-bot please reply to everyone",
        attachment_ids: &[],
        suppress_event: false,
        run_id: None,
    })?;

    let tmpdir = tempfile::tempdir()?;
    let spec = AgentSpec {
        display_name: "Gemini Live Bot".to_string(),
        description: None,
        system_prompt: None,
        model: model.clone(),
        reasoning_effort: None,
        env_vars: vec![],
        working_directory: tmpdir.path().to_path_buf(),
        bridge_endpoint: bridge_url.clone(),
    };

    let driver = GeminiDriver;
    let attach_result = driver
        .open_session(agent_key.to_string(), spec, SessionIntent::New)
        .await?;
    let mut handle = attach_result.session;

    let prompt = PromptReq {
        text: format!(
            "You are an agent named `{agent_key}` in a chat channel. \
             Use the `send_message` tool to post the exact text \
             `hello world` to the channel `#general`. Do not include any \
             other commentary — just call the tool once and stop."
        ),
        attachments: vec![],
    };

    handle.run(Some(prompt)).await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        if messages
            .iter()
            .any(|m| m.sender_name == agent_key && m.content.to_lowercase().contains("hello world"))
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let _ = handle.close().await;
    bridge_ct.cancel();

    if !found {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        let history_str = format!(
            "{:#?}",
            messages
                .iter()
                .map(|m| (&m.sender_name, &m.content))
                .collect::<Vec<_>>()
        );
        let diagnostics = collect_failure_diagnostics("gemini", None, tmpdir.path(), &history_str);
        anyhow::bail!(
            "agent did not send a reply containing 'hello world' within 60s{}",
            diagnostics
        );
    }

    Ok(())
}

/// Live round-trip: spawn a real `kimi` (Moonshot Kimi CLI) runtime, send it a
/// prompt, verify its reply lands in the Chorus store via the shared bridge.
///
/// Requires:
///   - `kimi` binary on PATH
///   - Moonshot credentials in `~/.kimi/credentials/kimi-code.json` (populated
///     by the Kimi CLI login flow; Chorus does not manage Kimi auth)
///   - Optional: `CHORUS_TEST_KIMI_MODEL` env var (defaults to
///     `kimi-code/kimi-for-coding`)
///
/// What it proves:
///   - `KimiDriver::attach` + `AgentHandle::start` with
///     `bridge_endpoint = Some(bridge_url)` writes a `.chorus-kimi-mcp.json`
///     config with a `"url"` field and passes the same URL inline in the ACP
///     `session/new` params, wiring the Kimi process to the shared bridge.
///   - The runtime's `send_message` MCP tool call routes through the bridge
///     into the Chorus store under the agent's identity.
#[tokio::test]
#[ignore = "requires kimi binary + Moonshot auth (~/.kimi/credentials/kimi-code.json)"]
async fn kimi_agent_replies_through_shared_bridge() -> anyhow::Result<()> {
    // 1. Skip if kimi binary not on PATH.
    if !binary_on_path("kimi") {
        eprintln!("SKIP: `kimi` binary not found on PATH");
        return Ok(());
    }

    let model = std::env::var("CHORUS_TEST_KIMI_MODEL")
        .unwrap_or_else(|_| "kimi-code/kimi-for-coding".to_string());

    // 2. Start Chorus server + shared bridge.
    let (server_url, store, tester_id) = start_chorus_server().await?;
    let (bridge_url, bridge_ct) = start_bridge_with_server(&server_url).await?;

    // 3. Seed agent record and channel membership.
    let agent_key = "kimi-live-bot";
    let agent_id = store.create_agent_record(&AgentRecordUpsert {
        name: agent_key,
        display_name: "Kimi Live Bot",
        description: None,
        system_prompt: None,
        runtime: "kimi",
        model: &model,
        reasoning_effort: None,
        machine_id: None,
        env_vars: &[],
    })?;
    join_channel_silent(&store, "general", &agent_id, "agent");

    // 4. Seed a user message in #general for conversation shape.
    store.create_message(CreateMessage {
        channel_name: "general",
        sender_id: &tester_id,
        sender_type: SenderType::Human,
        content: "@kimi-live-bot please reply to everyone",
        attachment_ids: &[],
        suppress_event: false,
        run_id: None,
    })?;

    // 5. Build AgentSpec with bridge_endpoint set.
    let tmpdir = tempfile::tempdir()?;
    let spec = AgentSpec {
        display_name: "Kimi Live Bot".to_string(),
        description: None,
        system_prompt: None,
        model: model.clone(),
        reasoning_effort: None,
        env_vars: vec![],
        working_directory: tmpdir.path().to_path_buf(),
        bridge_endpoint: bridge_url.clone(),
    };

    // 6. Open a new session and run the runtime with initial prompt.
    //    Kimi uses ACP over stdio (`kimi acp`). The driver writes a
    //    `.chorus-kimi-mcp.json` config file AND embeds the bridge URL in the
    //    ACP `session/new` params — both touch points are exercised here.
    let driver = KimiDriver;
    let attach_result = driver
        .open_session(agent_key.to_string(), spec, SessionIntent::New)
        .await?;
    let mut handle = attach_result.session;

    let prompt = PromptReq {
        text: format!(
            "You are an agent named `{agent_key}` in a chat channel. \
             Use the `send_message` tool to post the exact text \
             `hello world` to the channel `#general`. Do not include any \
             other commentary — just call the tool once and stop."
        ),
        attachments: vec![],
    };

    handle.run(Some(prompt)).await?;

    // 7. Poll the store for up to 60 seconds waiting for the agent's reply.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        if messages
            .iter()
            .any(|m| m.sender_name == agent_key && m.content.to_lowercase().contains("hello world"))
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // 8. Clean up.
    let _ = handle.close().await;
    bridge_ct.cancel();

    if !found {
        let (messages, _) = store.get_history("general", 100, None, None)?;
        let history_str = format!(
            "{:#?}",
            messages
                .iter()
                .map(|m| (&m.sender_name, &m.content))
                .collect::<Vec<_>>()
        );
        let diagnostics = collect_failure_diagnostics(
            "kimi",
            runtime_log_path("kimi").as_deref(),
            tmpdir.path(),
            &history_str,
        );
        anyhow::bail!(
            "agent did not send a reply containing 'hello world' within 60s{}",
            diagnostics
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Diagnostic-helper unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod diagnostic_helper_tests {
    use super::*;

    #[test]
    fn redact_pairing_tokens_masks_token_segment() {
        let input = r#"{"url": "http://127.0.0.1:4321/token/abc123xyz/mcp"}"#;
        let out = redact_pairing_tokens(input);
        assert!(out.contains("/token/[REDACTED]/mcp"));
        assert!(!out.contains("abc123xyz"));
    }

    #[test]
    fn redact_pairing_tokens_leaves_non_url_text_alone() {
        let input = "normal text with no URL";
        assert_eq!(redact_pairing_tokens(input), input);
    }

    #[test]
    fn redact_pairing_tokens_handles_multiple_tokens() {
        let input = "A=/token/tok1/mcp B=/token/tok2/mcp";
        let out = redact_pairing_tokens(input);
        assert_eq!(out, "A=/token/[REDACTED]/mcp B=/token/[REDACTED]/mcp");
    }

    #[test]
    fn read_log_tail_returns_full_file_when_smaller_than_cap() {
        let tmp =
            std::env::temp_dir().join(format!("chorus_log_tail_small_{}.log", std::process::id()));
        std::fs::write(&tmp, b"line one\nline two\n").unwrap();
        let got = read_log_tail(&tmp, 1024).unwrap();
        assert_eq!(got, "line one\nline two\n");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn read_log_tail_trims_to_newline_when_seeking() {
        let tmp =
            std::env::temp_dir().join(format!("chorus_log_tail_big_{}.log", std::process::id()));
        let mut contents = String::new();
        for i in 0..500 {
            contents.push_str(&format!("line number {i:04}\n"));
        }
        std::fs::write(&tmp, contents.as_bytes()).unwrap();
        let got = read_log_tail(&tmp, 128).unwrap();
        assert!(got.len() <= 128);
        // First char must be the start of a line (either the original line
        // we landed on, or the start of the next).
        assert!(
            !got.starts_with("umber"),
            "must not start mid-line: {got:?}"
        );
        let _ = std::fs::remove_file(&tmp);
    }
}
