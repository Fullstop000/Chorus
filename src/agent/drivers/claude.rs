//! v2 Claude driver backed by Claude headless CLI.

use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{bail, Context};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use super::claude_headless::{self, HeadlessEvent};
use crate::agent::AgentRuntime;
use crate::utils::cmd::{command_exists, run_command};

use super::*;

// ---------------------------------------------------------------------------
// MCP config construction
// ---------------------------------------------------------------------------

/// Build the `mcpServers.chat` config block for `.chorus-claude-mcp.json`.
///
/// Produces the native HTTP MCP shape, connecting the runtime to the shared
/// bridge at `{endpoint}/token/{token}/mcp`. Factored out so config-shape
/// tests don't need a live bridge.
fn build_mcp_config(bridge_endpoint: &str, token: &str) -> serde_json::Value {
    let url = crate::bridge::token_mcp_url(bridge_endpoint, token);
    serde_json::json!({
        "mcpServers": {
            "chat": {
                "type": "http",
                "url": url
            }
        }
    })
}

// ---------------------------------------------------------------------------
// ClaudeDriver — RuntimeDriver
// ---------------------------------------------------------------------------

pub struct ClaudeDriver;

#[async_trait]
impl RuntimeDriver for ClaudeDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Claude
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        if !command_exists("claude") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::StreamJson,
                capabilities: CapabilitySet::MODEL_LIST | CapabilitySet::LOGIN,
            });
        }

        let auth = run_command("claude", &["auth", "status"])
            .ok()
            .and_then(|result| {
                if !result.success {
                    return Some(ProbeAuth::Unauthed);
                }
                let payload: serde_json::Value = serde_json::from_str(&result.stdout).ok()?;
                Some(if payload["loggedIn"].as_bool().unwrap_or(false) {
                    ProbeAuth::Authed
                } else {
                    ProbeAuth::Unauthed
                })
            })
            .unwrap_or(ProbeAuth::Unauthed);

        Ok(RuntimeProbe {
            auth,
            transport: TransportKind::StreamJson,
            capabilities: CapabilitySet::MODEL_LIST | CapabilitySet::LOGIN,
        })
    }

    async fn login(&self) -> anyhow::Result<LoginOutcome> {
        Ok(LoginOutcome::PendingUserAction {
            message: "Run 'claude login' to authenticate".into(),
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo::from_id("sonnet".into()),
            ModelInfo::from_id("opus".into()),
            ModelInfo::from_id("haiku".into()),
        ])
    }

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(vec![])
    }

    async fn attach(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let (events, event_tx) = EventFanOut::new();
        let handle = ClaudeHandle {
            key,
            state: AgentState::Idle,
            events: events.clone(),
            event_tx,
            spec,
            child: None,
            stdin_tx: None,
            shared: None,
            reader_handles: Vec::new(),
        };
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }
}

// ---------------------------------------------------------------------------
// ClaudeHandle — AgentHandle
// ---------------------------------------------------------------------------

pub struct ClaudeHandle {
    key: AgentKey,
    state: AgentState,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    spec: AgentSpec,
    child: Option<std::process::Child>,
    stdin_tx: Option<mpsc::Sender<String>>,
    shared: Option<Arc<std::sync::Mutex<SharedReaderState>>>,
    reader_handles: Vec<tokio::task::JoinHandle<()>>,
}

/// Mutable state shared between the handle and the stdout reader task.
struct SharedReaderState {
    session_id: Option<String>,
    run_id: Option<RunId>,
    agent_state: AgentState,
}

impl ClaudeHandle {
    fn emit(&self, event: DriverEvent) {
        if let Err(e) = self.event_tx.try_send(event) {
            warn!("claude v2: failed to emit event: {e}");
        }
    }
}

#[async_trait]
impl AgentHandle for ClaudeHandle {
    fn key(&self) -> &AgentKey {
        &self.key
    }

    fn state(&self) -> AgentState {
        if let Some(ref shared) = self.shared {
            shared.lock().unwrap().agent_state.clone()
        } else {
            self.state.clone()
        }
    }

    async fn start(
        &mut self,
        _opts: StartOpts,
        init_prompt: Option<PromptReq>,
    ) -> anyhow::Result<()> {
        if !matches!(self.state, AgentState::Idle) {
            bail!("claude v2: start called in non-idle state");
        }

        self.state = AgentState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        // Write MCP config file
        let wd = &self.spec.working_directory;
        let mcp_config_path = wd.join(".chorus-claude-mcp.json");
        let token = super::request_pairing_token(&self.spec.bridge_endpoint, &self.key)
            .await
            .context("failed to pair with shared bridge")?;
        let mcp_config = build_mcp_config(&self.spec.bridge_endpoint, &token);
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)
            .context("failed to write MCP config")?;

        // Build CLI args
        let mcp_path_str = mcp_config_path.to_string_lossy().into_owned();
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--input-format".into(),
            "stream-json".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--include-partial-messages".into(),
            "--permission-mode".into(),
            "acceptEdits".into(),
            "--allowedTools".into(),
            "Bash,Read,Edit,Write,MultiEdit,Glob,Grep,LS,mcp__chat__*".into(),
            "--mcp-config".into(),
            mcp_path_str,
        ];
        if !self.spec.model.is_empty() {
            args.push("--model".into());
            args.push(self.spec.model.clone());
        }
        if let Some(ref prompt) = self.spec.system_prompt {
            args.push("--append-system-prompt".into());
            args.push(prompt.clone());
        }

        let mut cmd = Command::new("claude");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Env: remove CLAUDECODE (prevents nested invocation detection)
        cmd.env_remove("CLAUDECODE");
        cmd.env("FORCE_COLOR", "0");
        for ev in &self.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut std_child = cmd.spawn().context("failed to spawn claude")?;

        let stdout = std_child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("claude v2: child has no stdout"))?;
        let stdout = tokio::process::ChildStdout::from_std(stdout)?;

        let stderr = std_child
            .stderr
            .take()
            .map(tokio::process::ChildStderr::from_std);
        let stdin_raw = std_child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("claude v2: child has no stdin"))?;
        let child_stdin = tokio::process::ChildStdin::from_std(stdin_raw)
            .context("claude v2: failed to convert stdin to async")?;

        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(64);
        self.reader_handles
            .push(spawn_stdin_writer(child_stdin, stdin_rx));

        // Claude CLI in stream-json mode requires stdin input before emitting
        // the system/init event. Send the initial prompt immediately so the
        // init event (and subsequent stream events) flow without deadlock.
        let initial_run_id = if let Some(ref req) = init_prompt {
            let rid = RunId::new_v4();
            let msg = claude_headless::build_user_message(&req.text);
            let _ = stdin_tx.send(format!("{msg}\n")).await;
            Some(rid)
        } else {
            None
        };

        let shared = Arc::new(std::sync::Mutex::new(SharedReaderState {
            session_id: None,
            run_id: initial_run_id,
            agent_state: AgentState::Starting,
        }));
        self.shared = Some(shared.clone());

        self.reader_handles.push(spawn_stdout_reader(
            self.key.clone(),
            self.event_tx.clone(),
            stdout,
            shared,
        ));

        if let Some(Ok(child_stderr)) = stderr {
            self.reader_handles.push(spawn_stderr_reader(child_stderr));
        }

        self.child = Some(std_child);
        self.stdin_tx = Some(stdin_tx);

        Ok(())
    }

    async fn new_session(&mut self) -> anyhow::Result<SessionId> {
        bail!("claude v2: new_session not yet implemented")
    }

    async fn resume_session(&mut self, _id: SessionId) -> anyhow::Result<()> {
        bail!("claude v2: resume_session not yet implemented")
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let session_id = if let Some(ref shared) = self.shared {
            let guard = shared.lock().unwrap();
            match &guard.agent_state {
                AgentState::Active { session_id } => session_id.clone(),
                _ => bail!("claude v2: prompt called in non-active state"),
            }
        } else {
            match &self.state {
                AgentState::Active { session_id } => session_id.clone(),
                _ => bail!("claude v2: prompt called in non-active state"),
            }
        };

        let run_id = RunId::new_v4();

        let msg = claude_headless::build_user_message(&req.text);

        let stdin_tx = self
            .stdin_tx
            .as_ref()
            .context("claude v2: stdin not available — handle not started")?;
        stdin_tx
            .send(format!("{msg}\n"))
            .await
            .map_err(|e| anyhow::anyhow!("claude v2: stdin write failed: {e}"))?;

        if let Some(ref shared) = self.shared {
            let mut guard = shared.lock().unwrap();
            guard.run_id = Some(run_id);
            guard.agent_state = AgentState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            };
        }
        self.state = AgentState::PromptInFlight {
            run_id,
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::PromptInFlight { run_id, session_id },
        });

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        // Claude v2 headless does not support in-flight cancellation; the
        // session must be closed and restarted. Return NotInFlight so the
        // caller receives a no-op rather than invalid session data.
        Ok(CancelOutcome::NotInFlight)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.state, AgentState::Closed) {
            return Ok(());
        }

        if let Some(ref mut child) = self.child {
            let _ = child.kill();
        }

        for handle in self.reader_handles.drain(..) {
            handle.abort();
        }

        if let Some(ref shared) = self.shared {
            shared.lock().unwrap().agent_state = AgentState::Closed;
        }
        self.state = AgentState::Closed;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Closed,
        });
        self.events.close();
        Ok(())
    }
}

impl Drop for ClaudeHandle {
    fn drop(&mut self) {
        if let Some(ref child) = self.child {
            let pid = child.id();
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

fn spawn_stdout_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    key: AgentKey,
    tx: mpsc::Sender<DriverEvent>,
    stdout: R,
    shared: Arc<std::sync::Mutex<SharedReaderState>>,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        // Tool call accumulator: (tool_id, tool_name, json_buffer, start_index)
        let mut pending_tool: Option<(String, String, String, u32)> = None;

        while let Ok(Some(line)) = lines.next_line().await {
            let parsed = claude_headless::parse_line(&line);
            trace!(key = %key, ?parsed, "claude headless line");

            match parsed {
                HeadlessEvent::SystemInit { session_id } => {
                    debug!(key = %key, session_id = %session_id, "claude headless: system init");

                    // Capture whether there's already a run in flight (initial
                    // prompt was sent on stdin before init arrived).
                    let pre_existing_run_id = {
                        let mut guard = shared.lock().unwrap();
                        guard.session_id = Some(session_id.clone());
                        guard.agent_state = AgentState::Active {
                            session_id: session_id.clone(),
                        };
                        guard.run_id
                    };

                    let _ = tx
                        .send(DriverEvent::SessionAttached {
                            key: key.clone(),
                            session_id: session_id.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(DriverEvent::Lifecycle {
                            key: key.clone(),
                            state: AgentState::Active {
                                session_id: session_id.clone(),
                            },
                        })
                        .await;

                    // If a prompt was already sent before init, emit
                    // PromptInFlight so the manager knows we're working.
                    if let Some(run_id) = pre_existing_run_id {
                        {
                            let mut guard = shared.lock().unwrap();
                            guard.agent_state = AgentState::PromptInFlight {
                                run_id,
                                session_id: session_id.clone(),
                            };
                        }
                        let _ = tx
                            .send(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::PromptInFlight {
                                    run_id,
                                    session_id: session_id.clone(),
                                },
                            })
                            .await;
                    }
                }

                HeadlessEvent::ApiRetry { attempt, error } => {
                    trace!(key = %key, attempt, %error, "claude headless: api retry");
                }

                HeadlessEvent::ThinkingDelta { text } => {
                    let run_id = shared.lock().unwrap().run_id;
                    if let Some(rid) = run_id {
                        let _ = tx
                            .send(DriverEvent::Output {
                                key: key.clone(),
                                run_id: rid,
                                item: AgentEventItem::Thinking { text },
                            })
                            .await;
                    }
                }

                HeadlessEvent::TextDelta { text } => {
                    let run_id = shared.lock().unwrap().run_id;
                    if let Some(rid) = run_id {
                        let _ = tx
                            .send(DriverEvent::Output {
                                key: key.clone(),
                                run_id: rid,
                                item: AgentEventItem::Text { text },
                            })
                            .await;
                    }
                }

                HeadlessEvent::ToolUseStart { index, id, name } => {
                    // Flush any pending tool call
                    let run_id = shared.lock().unwrap().run_id;
                    if let Some(rid) = run_id {
                        if let Some((_tid, tname, tbuf, _idx)) = pending_tool.take() {
                            let input: serde_json::Value =
                                serde_json::from_str(&tbuf).unwrap_or(serde_json::Value::Null);
                            let _ = tx
                                .send(DriverEvent::Output {
                                    key: key.clone(),
                                    run_id: rid,
                                    item: AgentEventItem::ToolCall { name: tname, input },
                                })
                                .await;
                        }
                    }
                    // Start new accumulation
                    pending_tool = Some((id, name, String::new(), index));
                }

                HeadlessEvent::InputJsonDelta { partial_json, .. } => {
                    if let Some(ref mut tool) = pending_tool {
                        tool.2.push_str(&partial_json);
                    }
                }

                HeadlessEvent::ContentBlockStop { index }
                | HeadlessEvent::ToolUseStop { index } => {
                    // If index matches current tool accumulation, flush it
                    let should_flush = pending_tool.as_ref().is_some_and(|t| t.3 == index);
                    if should_flush {
                        let run_id = shared.lock().unwrap().run_id;
                        if let Some(rid) = run_id {
                            if let Some((_tid, tname, tbuf, _idx)) = pending_tool.take() {
                                let input: serde_json::Value =
                                    serde_json::from_str(&tbuf).unwrap_or(serde_json::Value::Null);
                                let _ = tx
                                    .send(DriverEvent::Output {
                                        key: key.clone(),
                                        run_id: rid,
                                        item: AgentEventItem::ToolCall { name: tname, input },
                                    })
                                    .await;
                            }
                        }
                    }
                }

                HeadlessEvent::TurnResult { session_id, .. } => {
                    let run_id = shared.lock().unwrap().run_id;
                    if let Some(rid) = run_id {
                        // Flush remaining tool calls
                        if let Some((_tid, tname, tbuf, _idx)) = pending_tool.take() {
                            let input: serde_json::Value =
                                serde_json::from_str(&tbuf).unwrap_or(serde_json::Value::Null);
                            let _ = tx
                                .send(DriverEvent::Output {
                                    key: key.clone(),
                                    run_id: rid,
                                    item: AgentEventItem::ToolCall { name: tname, input },
                                })
                                .await;
                        }

                        let resolved_sid = if session_id.is_empty() {
                            shared
                                .lock()
                                .unwrap()
                                .session_id
                                .clone()
                                .unwrap_or_default()
                        } else {
                            session_id
                        };

                        let _ = tx
                            .send(DriverEvent::Output {
                                key: key.clone(),
                                run_id: rid,
                                item: AgentEventItem::TurnEnd,
                            })
                            .await;
                        let _ = tx
                            .send(DriverEvent::Completed {
                                key: key.clone(),
                                run_id: rid,
                                result: RunResult {
                                    session_id: resolved_sid.clone(),
                                    finish_reason: FinishReason::Natural,
                                },
                            })
                            .await;
                        {
                            let mut guard = shared.lock().unwrap();
                            guard.run_id = None;
                            guard.agent_state = AgentState::Active {
                                session_id: resolved_sid.clone(),
                            };
                        }

                        let _ = tx
                            .send(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::Active {
                                    session_id: resolved_sid,
                                },
                            })
                            .await;
                    }
                }

                HeadlessEvent::Unknown => {}
            }
        }

        // stdout EOF
        {
            let mut guard = shared.lock().unwrap();
            guard.agent_state = AgentState::Closed;
        }
        let _ = tx
            .send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Closed,
            })
            .await;
    })
}

fn spawn_stderr_reader(stderr: tokio::process::ChildStderr) -> tokio::task::JoinHandle<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            debug!(target: "claude_v2_stderr", "{}", line);
        }
    })
}

fn spawn_stdin_writer(
    mut stdin: tokio::process::ChildStdin,
    mut rx: mpsc::Receiver<String>,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::AsyncWriteExt;

    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if let Err(e) = stdin.write_all(data.as_bytes()).await {
                warn!("claude v2: stdin write failed: {e}");
                break;
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test-claude".into(),
            description: None,
            system_prompt: None,
            model: "sonnet".into(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_endpoint: "http://127.0.0.1:1".into(),
        }
    }

    #[tokio::test]
    async fn test_claude_driver_probe_not_installed() {
        // claude binary is not on PATH in CI/test environments
        let driver = ClaudeDriver;
        let probe = driver.probe().await.unwrap();
        // Either NotInstalled or Unauthed depending on host — both are valid.
        // The key invariant: transport and capabilities are always set.
        assert_eq!(probe.transport, TransportKind::StreamJson);
        assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
        assert!(probe.capabilities.contains(CapabilitySet::LOGIN));
    }

    #[tokio::test]
    async fn test_claude_driver_list_models() {
        let driver = ClaudeDriver;
        let models = driver.list_models().await.unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "sonnet");
        assert_eq!(models[1].id, "opus");
        assert_eq!(models[2].id, "haiku");
    }

    #[tokio::test]
    async fn test_claude_driver_attach_returns_idle() {
        let driver = ClaudeDriver;
        let result = driver
            .attach("agent-claude-1".into(), test_spec())
            .await
            .unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }

    // ---- build_mcp_config tests ----

    #[test]
    fn build_mcp_config_http_shape() {
        // Native HTTP MCP shape — the only shape we produce.
        let config = build_mcp_config("http://127.0.0.1:4321", "tok-xyz");
        let chat = &config["mcpServers"]["chat"];
        assert_eq!(chat["type"], "http");
        assert_eq!(chat["url"], "http://127.0.0.1:4321/token/tok-xyz/mcp");
        assert!(chat.get("command").is_none());
        assert!(chat.get("args").is_none());
    }

    #[test]
    fn build_mcp_config_trims_trailing_slash() {
        // Endpoint with trailing slash must not produce `//token/` in the URL.
        let config = build_mcp_config("http://127.0.0.1:4321/", "tok-xyz");
        let chat = &config["mcpServers"]["chat"];
        assert_eq!(chat["url"], "http://127.0.0.1:4321/token/tok-xyz/mcp");
    }

    /// Feed captured JSONL through spawn_stdout_reader via a mock pipe and
    /// verify the DriverEvent sequence matches expectations.
    #[tokio::test]
    async fn test_stdout_reader_full_turn() {
        use tokio::io::AsyncWriteExt;
        use tokio::time::{timeout, Duration};

        // Captured JSONL: system init → thinking → text → tool_use → result
        let jsonl = [
            r#"{"type":"system","subtype":"init","session_id":"sess-test","tools":["Bash"],"mcp_servers":[],"model":"claude-sonnet-4-6"}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hi"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"Read","input":{}}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"file\":"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"main.rs\"}"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":2}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Hi","stop_reason":"end_turn","session_id":"sess-test","duration_ms":500}"#,
        ];

        // Create a duplex stream as mock stdout
        let (mock_stdout, mut writer) = tokio::io::duplex(4096);

        // Set up shared state and channels
        let (event_tx, mut event_rx) = mpsc::channel::<DriverEvent>(64);
        let initial_run_id = RunId::new_v4();
        let shared = Arc::new(std::sync::Mutex::new(SharedReaderState {
            session_id: None,
            run_id: Some(initial_run_id),
            agent_state: AgentState::Starting,
        }));

        // Spawn the reader
        let _handle =
            spawn_stdout_reader("test-agent".into(), event_tx, mock_stdout, shared.clone());

        // Write JSONL lines
        for line in &jsonl {
            writer.write_all(line.as_bytes()).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
        }
        // Close writer to trigger EOF
        drop(writer);

        // Collect events (with timeout)
        let mut events = Vec::new();
        while let Ok(Some(ev)) = timeout(Duration::from_secs(2), event_rx.recv()).await {
            events.push(ev);
        }

        // Verify event sequence
        // 1. SessionAttached
        assert!(
            matches!(&events[0], DriverEvent::SessionAttached { session_id, .. } if session_id == "sess-test"),
            "expected SessionAttached, got {:?}",
            events[0]
        );
        // 2. Lifecycle(Active)
        assert!(
            matches!(&events[1], DriverEvent::Lifecycle { state: AgentState::Active { session_id }, .. } if session_id == "sess-test"),
            "expected Lifecycle(Active), got {:?}",
            events[1]
        );
        // 3. Lifecycle(PromptInFlight) — from deferred prompt
        assert!(
            matches!(
                &events[2],
                DriverEvent::Lifecycle {
                    state: AgentState::PromptInFlight { .. },
                    ..
                }
            ),
            "expected Lifecycle(PromptInFlight), got {:?}",
            events[2]
        );
        // 4. Thinking delta
        assert!(
            matches!(&events[3], DriverEvent::Output { item: AgentEventItem::Thinking { text }, .. } if text == "hmm"),
            "expected Thinking, got {:?}",
            events[3]
        );
        // 5. Text delta
        assert!(
            matches!(&events[4], DriverEvent::Output { item: AgentEventItem::Text { text }, .. } if text == "Hi"),
            "expected Text, got {:?}",
            events[4]
        );
        // 6. ToolCall (flushed on content_block_stop)
        match &events[5] {
            DriverEvent::Output {
                item: AgentEventItem::ToolCall { name, input },
                ..
            } => {
                assert_eq!(name, "Read");
                assert_eq!(input["file"], "main.rs");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        // 7. TurnEnd
        assert!(
            matches!(
                &events[6],
                DriverEvent::Output {
                    item: AgentEventItem::TurnEnd,
                    ..
                }
            ),
            "expected TurnEnd, got {:?}",
            events[6]
        );
        // 8. Completed
        assert!(
            matches!(
                &events[7],
                DriverEvent::Completed {
                    result: RunResult {
                        finish_reason: FinishReason::Natural,
                        ..
                    },
                    ..
                }
            ),
            "expected Completed, got {:?}",
            events[7]
        );
        // 9. Lifecycle(Active) — reset after completion
        assert!(
            matches!(
                &events[8],
                DriverEvent::Lifecycle {
                    state: AgentState::Active { .. },
                    ..
                }
            ),
            "expected Lifecycle(Active), got {:?}",
            events[8]
        );
        // 10. Lifecycle(Closed) — EOF
        assert!(
            matches!(
                &events[9],
                DriverEvent::Lifecycle {
                    state: AgentState::Closed,
                    ..
                }
            ),
            "expected Lifecycle(Closed), got {:?}",
            events[9]
        );
        assert_eq!(events.len(), 10);

        // Verify shared state ended as Closed
        let final_state = shared.lock().unwrap().agent_state.clone();
        assert!(matches!(final_state, AgentState::Closed));
    }
}
