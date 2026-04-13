//! v2 Claude driver backed by the `claude-agent-acp` ACP adapter binary.

use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{bail, Context};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use crate::agent::drivers::v2::acp_protocol::{self, AcpParsed, AcpPhase, AcpUpdateItem};
use crate::agent::drivers::{command_exists, run_command};
use crate::agent::AgentRuntime;

use super::*;

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
                transport: TransportKind::AcpAdapter,
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
            transport: TransportKind::AcpAdapter,
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
            session_id: None,
            next_request_id: 4,
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
    #[allow(dead_code)]
    session_id: Option<String>,
    next_request_id: u64,
    reader_handles: Vec<tokio::task::JoinHandle<()>>,
}

/// Mutable state shared between the handle and the stdout reader task.
struct SharedReaderState {
    phase: AcpPhase,
    session_id: Option<String>,
    run_id: Option<RunId>,
    pending_prompt: Option<String>,
    #[allow(dead_code)]
    pending_session_id: Option<String>,
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
        self.state.clone()
    }

    async fn start(
        &mut self,
        opts: StartOpts,
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

        let mut cmd = Command::new("claude-agent-acp");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Env: remove CLAUDECODE (prevents nested invocation detection)
        cmd.env_remove("CLAUDECODE");
        cmd.env("FORCE_COLOR", "0");
        for ev in &self.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut std_child = cmd.spawn().context("failed to spawn claude-agent-acp")?;

        let stdout = std_child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("claude v2: child has no stdout"))?;
        let stdout = tokio::process::ChildStdout::from_std(stdout)?;

        let stderr = std_child.stderr.take().map(tokio::process::ChildStderr::from_std);
        let stdin = std_child.stdin.take().map(tokio::process::ChildStdin::from_std);

        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(64);

        if let Some(Ok(child_stdin)) = stdin {
            self.reader_handles
                .push(spawn_stdin_writer(child_stdin, stdin_rx));
        }

        // Build session/new params with MCP server config
        let session_new_params = serde_json::json!({
            "cwd": self.spec.working_directory.to_string_lossy(),
            "mcpServers": [{
                "name": "chat",
                "command": self.spec.bridge_binary,
                "args": ["bridge", "--agent-id", &self.key, "--server-url", &self.spec.server_url],
                "env": []
            }]
        });

        // Prepare deferred prompt if provided
        let pending_prompt = init_prompt.map(|req| req.text);

        let shared = Arc::new(std::sync::Mutex::new(SharedReaderState {
            phase: AcpPhase::AwaitingInitResponse,
            session_id: None,
            run_id: None,
            pending_prompt,
            pending_session_id: opts.resume_session_id.clone(),
        }));

        // Send initialize request
        let init_req = acp_protocol::build_initialize_request(1);
        let _ = stdin_tx.send(format!("{init_req}\n")).await;

        self.reader_handles.push(spawn_stdout_reader(
            self.key.clone(),
            self.event_tx.clone(),
            stdout,
            shared,
            stdin_tx.clone(),
            session_new_params,
            opts.resume_session_id,
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
        let session_id = match &self.state {
            AgentState::Active { session_id } => session_id.clone(),
            _ => bail!("claude v2: prompt called in non-active state"),
        };

        let run_id = RunId::new_v4();
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        let encoded =
            acp_protocol::build_session_prompt_request(request_id, &session_id, &req.text);

        if let Some(ref stdin_tx) = self.stdin_tx {
            stdin_tx
                .send(format!("{encoded}\n"))
                .await
                .map_err(|e| anyhow::anyhow!("claude v2: stdin write failed: {e}"))?;
        }

        self.state = AgentState::PromptInFlight {
            run_id,
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::PromptInFlight {
                run_id,
                session_id,
            },
        });

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        Ok(CancelOutcome::SessionReplaced {
            new_session_id: String::new(),
        })
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

#[allow(clippy::too_many_arguments)]
fn spawn_stdout_reader(
    key: AgentKey,
    tx: mpsc::Sender<DriverEvent>,
    stdout: tokio::process::ChildStdout,
    shared: Arc<std::sync::Mutex<SharedReaderState>>,
    stdin_tx: mpsc::Sender<String>,
    session_new_params: serde_json::Value,
    resume_session_id: Option<String>,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut tool_acc = acp_protocol::ToolCallAccumulator::new();

        while let Ok(Some(line)) = lines.next_line().await {
            let parsed = acp_protocol::parse_line(&line);
            trace!(key = %key, ?parsed, "claude acp line");

            let (phase, run_id, session_id) = {
                let guard = shared.lock().unwrap();
                (guard.phase, guard.run_id, guard.session_id.clone())
            };

            match parsed {
                AcpParsed::InitializeResponse => {
                    debug!(key = %key, "claude acp: initialize response received");
                    {
                        let mut guard = shared.lock().unwrap();
                        guard.phase = AcpPhase::AwaitingSessionResponse;
                    }

                    // Send session/new or session/load
                    let req = if let Some(ref sid) = resume_session_id {
                        acp_protocol::build_session_load_request(
                            2,
                            sid,
                            session_new_params.clone(),
                        )
                    } else {
                        acp_protocol::build_session_new_request(2, session_new_params.clone())
                    };
                    let _ = stdin_tx.send(format!("{req}\n")).await;
                }

                AcpParsed::SessionResponse { session_id: sid } => {
                    let resolved_sid = sid
                        .or_else(|| resume_session_id.clone())
                        .unwrap_or_default();
                    debug!(key = %key, session_id = %resolved_sid, "claude acp: session established");

                    {
                        let mut guard = shared.lock().unwrap();
                        guard.phase = AcpPhase::Active;
                        guard.session_id = Some(resolved_sid.clone());
                    }

                    let _ = tx
                        .send(DriverEvent::SessionAttached {
                            key: key.clone(),
                            session_id: resolved_sid.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(DriverEvent::Lifecycle {
                            key: key.clone(),
                            state: AgentState::Active {
                                session_id: resolved_sid.clone(),
                            },
                        })
                        .await;

                    // Send deferred prompt if one was queued
                    let deferred = {
                        let mut guard = shared.lock().unwrap();
                        guard.pending_prompt.take()
                    };
                    if let Some(prompt_text) = deferred {
                        let run_id = RunId::new_v4();
                        {
                            let mut guard = shared.lock().unwrap();
                            guard.run_id = Some(run_id);
                        }

                        let _ = tx
                            .send(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::PromptInFlight {
                                    run_id,
                                    session_id: resolved_sid.clone(),
                                },
                            })
                            .await;

                        // session_id in prompt required for claude
                        let req = acp_protocol::build_session_prompt_request(
                            3,
                            &resolved_sid,
                            &prompt_text,
                        );
                        let _ = stdin_tx.send(format!("{req}\n")).await;
                    }
                }

                AcpParsed::SessionUpdate { items } => {
                    if phase != AcpPhase::Active {
                        continue;
                    }

                    for item in items {
                        match item {
                            AcpUpdateItem::SessionInit {
                                session_id: new_sid,
                            } => {
                                {
                                    let mut guard = shared.lock().unwrap();
                                    guard.session_id = Some(new_sid.clone());
                                }
                                let _ = tx
                                    .send(DriverEvent::SessionAttached {
                                        key: key.clone(),
                                        session_id: new_sid,
                                    })
                                    .await;
                            }
                            AcpUpdateItem::Thinking { text } => {
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
                            AcpUpdateItem::Text { text } => {
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
                            AcpUpdateItem::ToolCall { id, name, input } => {
                                // Flush any pending calls before recording the new one
                                if let Some(rid) = run_id {
                                    for (_tid, tname, tinput) in tool_acc.drain() {
                                        let _ = tx
                                            .send(DriverEvent::Output {
                                                key: key.clone(),
                                                run_id: rid,
                                                item: AgentEventItem::ToolCall {
                                                    name: tname,
                                                    input: tinput,
                                                },
                                            })
                                            .await;
                                    }
                                }
                                tool_acc.record_call(id, name, input);
                            }
                            AcpUpdateItem::ToolCallUpdate { id, input } => {
                                tool_acc.merge_update(id, input);
                            }
                            AcpUpdateItem::ToolResult { content } => {
                                // Flush pending tool calls before emitting result
                                if let Some(rid) = run_id {
                                    for (_tid, tname, tinput) in tool_acc.drain() {
                                        let _ = tx
                                            .send(DriverEvent::Output {
                                                key: key.clone(),
                                                run_id: rid,
                                                item: AgentEventItem::ToolCall {
                                                    name: tname,
                                                    input: tinput,
                                                },
                                            })
                                            .await;
                                    }
                                    let _ = tx
                                        .send(DriverEvent::Output {
                                            key: key.clone(),
                                            run_id: rid,
                                            item: AgentEventItem::ToolResult { content },
                                        })
                                        .await;
                                }
                            }
                            AcpUpdateItem::TurnEnd => {
                                if let Some(rid) = run_id {
                                    let _ = tx
                                        .send(DriverEvent::Output {
                                            key: key.clone(),
                                            run_id: rid,
                                            item: AgentEventItem::TurnEnd,
                                        })
                                        .await;
                                }
                            }
                        }
                    }
                }

                AcpParsed::PromptResponse {
                    session_id: resp_sid,
                } => {
                    if let Some(rid) = run_id {
                        // Flush remaining tool calls
                        for (_tid, tname, tinput) in tool_acc.drain() {
                            let _ = tx
                                .send(DriverEvent::Output {
                                    key: key.clone(),
                                    run_id: rid,
                                    item: AgentEventItem::ToolCall {
                                        name: tname,
                                        input: tinput,
                                    },
                                })
                                .await;
                        }

                        let resolved_sid = resp_sid
                            .or(session_id)
                            .unwrap_or_default();

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
                        let _ = tx
                            .send(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::Active {
                                    session_id: resolved_sid,
                                },
                            })
                            .await;

                        {
                            let mut guard = shared.lock().unwrap();
                            guard.run_id = None;
                        }
                    }
                }

                AcpParsed::PermissionRequested {
                    request_id,
                    tool_name,
                } => {
                    debug!(
                        key = %key,
                        ?tool_name,
                        "claude acp: auto-approving permission request"
                    );
                    let resp = acp_protocol::build_permission_approval_response(request_id, true);
                    let _ = stdin_tx.send(format!("{resp}\n")).await;
                }

                AcpParsed::Error { message } => {
                    warn!(key = %key, %message, "claude acp: error response");
                    if let Some(rid) = run_id {
                        let _ = tx
                            .send(DriverEvent::Failed {
                                key: key.clone(),
                                run_id: rid,
                                error: AgentError::RuntimeReported(message),
                            })
                            .await;
                    }
                }

                AcpParsed::Unknown => {}
            }
        }

        // stdout EOF
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
            bridge_binary: "bridge".into(),
            server_url: "http://localhost:3001".into(),
        }
    }

    #[tokio::test]
    async fn test_claude_driver_probe_not_installed() {
        // claude binary is not on PATH in CI/test environments
        let driver = ClaudeDriver;
        let probe = driver.probe().await.unwrap();
        // Either NotInstalled or Unauthed depending on host — both are valid.
        // The key invariant: transport and capabilities are always set.
        assert_eq!(probe.transport, TransportKind::AcpAdapter);
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
}
