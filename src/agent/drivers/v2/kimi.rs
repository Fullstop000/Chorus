//! Native v2 driver for the Kimi runtime using ACP protocol.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use crate::agent::drivers::{command_exists, home_dir, read_file};
use crate::agent::AgentRuntime;

use super::acp_protocol::{self, AcpParsed, AcpPhase, AcpUpdateItem, ToolCallAccumulator};
use super::*;

// ---------------------------------------------------------------------------
// KimiDriver
// ---------------------------------------------------------------------------

pub struct KimiDriver;

#[async_trait]
impl RuntimeDriver for KimiDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Kimi
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

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(vec![])
    }

    async fn attach(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let (events, event_tx) = EventFanOut::new();
        let handle = KimiHandle {
            key,
            state: AgentState::Idle,
            events: events.clone(),
            event_tx,
            spec,
            child: None,
            stdin_tx: None,
            shared: Arc::new(Mutex::new(SharedReaderState {
                phase: AcpPhase::AwaitingInitResponse,
                session_id: None,
                run_id: None,
                pending_prompt: None,
                pending_session_id: None,
                agent_state: AgentState::Idle,
            })),
            next_request_id: 4,
            reader_handles: vec![],
        };
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }
}

// ---------------------------------------------------------------------------
// Shared reader state
// ---------------------------------------------------------------------------

struct SharedReaderState {
    phase: AcpPhase,
    session_id: Option<String>,
    run_id: Option<RunId>,
    pending_prompt: Option<String>,
    /// Kimi omits sessionId from session/load responses; stash the requested
    /// id so the reader task can fall back to it.
    pending_session_id: Option<String>,
    /// Canonical agent state, shared between handle methods and the reader task.
    agent_state: AgentState,
}

// ---------------------------------------------------------------------------
// KimiHandle
// ---------------------------------------------------------------------------

pub struct KimiHandle {
    key: AgentKey,
    state: AgentState,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    spec: AgentSpec,
    child: Option<std::process::Child>,
    stdin_tx: Option<mpsc::Sender<String>>,
    shared: Arc<Mutex<SharedReaderState>>,
    next_request_id: u64,
    reader_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl KimiHandle {
    fn emit(&self, event: DriverEvent) {
        let _ = self.event_tx.try_send(event);
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }
}

impl Drop for KimiHandle {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            let pid = child.id();
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
    }
}

#[async_trait]
impl AgentHandle for KimiHandle {
    fn key(&self) -> &AgentKey {
        &self.key
    }

    fn state(&self) -> AgentState {
        self.shared.lock().unwrap().agent_state.clone()
    }

    async fn start(
        &mut self,
        opts: StartOpts,
        init_prompt: Option<PromptReq>,
    ) -> anyhow::Result<()> {
        {
            let mut s = self.shared.lock().unwrap();
            s.agent_state = AgentState::Starting;
        }
        self.state = AgentState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        // Write MCP config file
        let wd = &self.spec.working_directory;
        let mcp_config_path = wd.join(".chorus-kimi-mcp.json");
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "chat": {
                    "command": &self.spec.bridge_binary,
                    "args": ["bridge", "--agent-id", &self.key, "--server-url", &self.spec.server_url]
                }
            }
        });
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)
            .context("failed to write MCP config")?;

        // Build CLI args
        let mcp_path_str = mcp_config_path.to_string_lossy().into_owned();
        let wd_str = wd.to_string_lossy().into_owned();
        let mut args = vec![
            "--work-dir".to_string(),
            wd_str,
            "--mcp-config-file".to_string(),
            mcp_path_str,
        ];
        if !self.spec.model.is_empty() {
            args.push("--model".to_string());
            args.push(self.spec.model.clone());
        }
        args.push("acp".to_string());

        // Build env
        let mut cmd = Command::new("kimi");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("FORCE_COLOR", "0")
            .env("NO_COLOR", "1");
        for ev in &self.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut child = cmd.spawn().context("failed to spawn kimi")?;
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;
        let mut stdin = child.stdin.take().context("missing stdin")?;

        // Write handshake synchronously before handing stdin to the async writer
        let init_req = acp_protocol::build_initialize_request(1);
        writeln!(stdin, "{init_req}").context("failed to write initialize request")?;

        let session_new_params = serde_json::json!({
            "cwd": self.spec.working_directory,
            "mcpServers": [{
                "name": "chat",
                "command": &self.spec.bridge_binary,
                "args": ["bridge", "--agent-id", &self.key, "--server-url", &self.spec.server_url],
                "env": []
            }]
        });

        let session_req = if let Some(ref sid) = opts.resume_session_id {
            {
                let mut shared = self.shared.lock().unwrap();
                shared.pending_session_id = Some(sid.clone());
            }
            acp_protocol::build_session_load_request(2, sid, session_new_params)
        } else {
            acp_protocol::build_session_new_request(2, session_new_params)
        };
        writeln!(stdin, "{session_req}").context("failed to write session request")?;

        // Stash deferred initial prompt
        if let Some(ref req) = init_prompt {
            let mut shared = self.shared.lock().unwrap();
            shared.pending_prompt = Some(req.text.clone());
        }

        // Stdin writer task
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
        self.stdin_tx = Some(stdin_tx.clone());
        let stdin_handle = tokio::task::spawn_blocking(move || {
            while let Some(line) = stdin_rx.blocking_recv() {
                if writeln!(stdin, "{line}").is_err() {
                    break;
                }
                if stdin.flush().is_err() {
                    break;
                }
            }
        });
        self.reader_handles.push(stdin_handle);

        // Stdout reader task
        let key = self.key.clone();
        let event_tx = self.event_tx.clone();
        let shared = self.shared.clone();
        let stdin_tx_for_reader = self.stdin_tx.clone().unwrap();
        let stdout_handle = tokio::spawn(async move {
            let reader = BufReader::new(tokio::process::ChildStdout::from_std(stdout).unwrap());
            let mut lines = reader.lines();
            let mut accumulator = ToolCallAccumulator::new();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                trace!(line = %line, "kimi stdout");
                let parsed = acp_protocol::parse_line(&line);

                match parsed {
                    AcpParsed::InitializeResponse => {
                        let mut s = shared.lock().unwrap();
                        s.phase = AcpPhase::AwaitingSessionResponse;
                        debug!("kimi: initialize response received");
                    }

                    AcpParsed::SessionResponse { session_id } => {
                        let (sid, deferred_prompt) = {
                            let mut s = shared.lock().unwrap();
                            s.phase = AcpPhase::Active;
                            let sid = session_id
                                .or_else(|| s.pending_session_id.take())
                                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                            s.session_id = Some(sid.clone());
                            s.agent_state = AgentState::Active {
                                session_id: sid.clone(),
                            };
                            let prompt = s.pending_prompt.take();
                            (sid, prompt)
                        };

                        let _ = event_tx.try_send(DriverEvent::SessionAttached {
                            key: key.clone(),
                            session_id: sid.clone(),
                        });
                        let _ = event_tx.try_send(DriverEvent::Lifecycle {
                            key: key.clone(),
                            state: AgentState::Active {
                                session_id: sid.clone(),
                            },
                        });

                        // Send deferred initial prompt now that we have a session
                        if let Some(prompt_text) = deferred_prompt {
                            let run_id = RunId::new_v4();
                            {
                                let mut s = shared.lock().unwrap();
                                s.run_id = Some(run_id);
                                s.agent_state = AgentState::PromptInFlight {
                                    run_id,
                                    session_id: sid.clone(),
                                };
                            }
                            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::PromptInFlight {
                                    run_id,
                                    session_id: sid.clone(),
                                },
                            });

                            let req =
                                acp_protocol::build_session_prompt_request(3, &sid, &prompt_text);
                            let _ = stdin_tx_for_reader.try_send(req);
                        }
                    }

                    AcpParsed::PromptResponse { .. } => {
                        let (run_id, sid) = {
                            let mut s = shared.lock().unwrap();
                            let rid = s.run_id.take();
                            let sid = s.session_id.clone().unwrap_or_default();
                            s.agent_state = AgentState::Active {
                                session_id: sid.clone(),
                            };
                            (rid, sid)
                        };

                        // Flush accumulated tool calls
                        if let Some(run_id) = run_id {
                            for (_id, name, input) in accumulator.drain() {
                                let _ = event_tx.try_send(DriverEvent::Output {
                                    key: key.clone(),
                                    run_id,
                                    item: AgentEventItem::ToolCall { name, input },
                                });
                            }

                            let _ = event_tx.try_send(DriverEvent::Output {
                                key: key.clone(),
                                run_id,
                                item: AgentEventItem::TurnEnd,
                            });
                            let _ = event_tx.try_send(DriverEvent::Completed {
                                key: key.clone(),
                                run_id,
                                result: RunResult {
                                    session_id: sid.clone(),
                                    finish_reason: FinishReason::Natural,
                                },
                            });
                            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::Active { session_id: sid },
                            });
                        }
                    }

                    AcpParsed::SessionUpdate { items } => {
                        let run_id = {
                            let s = shared.lock().unwrap();
                            s.run_id
                        };
                        let Some(run_id) = run_id else { continue };

                        for item in items {
                            match item {
                                AcpUpdateItem::SessionInit { session_id } => {
                                    let mut s = shared.lock().unwrap();
                                    s.session_id = Some(session_id);
                                }
                                AcpUpdateItem::Thinking { text } => {
                                    let _ = event_tx.try_send(DriverEvent::Output {
                                        key: key.clone(),
                                        run_id,
                                        item: AgentEventItem::Thinking { text },
                                    });
                                }
                                AcpUpdateItem::Text { text } => {
                                    let _ = event_tx.try_send(DriverEvent::Output {
                                        key: key.clone(),
                                        run_id,
                                        item: AgentEventItem::Text { text },
                                    });
                                }
                                AcpUpdateItem::ToolCall { id, name, input } => {
                                    // Flush any previous pending calls before recording new one
                                    for (_id, n, inp) in accumulator.drain() {
                                        let _ = event_tx.try_send(DriverEvent::Output {
                                            key: key.clone(),
                                            run_id,
                                            item: AgentEventItem::ToolCall {
                                                name: n,
                                                input: inp,
                                            },
                                        });
                                    }
                                    accumulator.record_call(id, name, input);
                                }
                                AcpUpdateItem::ToolCallUpdate { id, input } => {
                                    accumulator.merge_update(id, input);
                                }
                                AcpUpdateItem::ToolResult { content } => {
                                    // Flush the accumulated tool call first
                                    for (_id, n, inp) in accumulator.drain() {
                                        let _ = event_tx.try_send(DriverEvent::Output {
                                            key: key.clone(),
                                            run_id,
                                            item: AgentEventItem::ToolCall {
                                                name: n,
                                                input: inp,
                                            },
                                        });
                                    }
                                    let _ = event_tx.try_send(DriverEvent::Output {
                                        key: key.clone(),
                                        run_id,
                                        item: AgentEventItem::ToolResult { content },
                                    });
                                }
                                AcpUpdateItem::TurnEnd => {
                                    for (_id, n, inp) in accumulator.drain() {
                                        let _ = event_tx.try_send(DriverEvent::Output {
                                            key: key.clone(),
                                            run_id,
                                            item: AgentEventItem::ToolCall {
                                                name: n,
                                                input: inp,
                                            },
                                        });
                                    }
                                    let _ = event_tx.try_send(DriverEvent::Output {
                                        key: key.clone(),
                                        run_id,
                                        item: AgentEventItem::TurnEnd,
                                    });
                                }
                            }
                        }
                    }

                    AcpParsed::PermissionRequested {
                        request_id,
                        tool_name,
                        options,
                    } => {
                        // Pick the most permissive option from the runtime's
                        // offered choices (allow_always > allow_once > first).
                        let option_id = acp_protocol::pick_best_option_id(&options);
                        debug!(
                            ?tool_name,
                            request_id, option_id, "kimi: auto-approving permission"
                        );
                        let response =
                            acp_protocol::build_permission_response_raw(request_id, option_id);
                        let _ = stdin_tx_for_reader.try_send(response);
                    }

                    AcpParsed::Error { message } => {
                        warn!(message = %message, "kimi: ACP error");
                        let run_id = {
                            let mut s = shared.lock().unwrap();
                            s.run_id.take()
                        };
                        if let Some(run_id) = run_id {
                            let _ = event_tx.try_send(DriverEvent::Failed {
                                key: key.clone(),
                                run_id,
                                error: AgentError::RuntimeReported(message),
                            });
                        }
                    }

                    AcpParsed::Unknown => {}
                }
            }

            // EOF — runtime exited
            let run_id = {
                let s = shared.lock().unwrap();
                s.run_id
            };
            if let Some(run_id) = run_id {
                let sid = shared
                    .lock()
                    .unwrap()
                    .session_id
                    .clone()
                    .unwrap_or_default();
                let _ = event_tx.try_send(DriverEvent::Completed {
                    key: key.clone(),
                    run_id,
                    result: RunResult {
                        session_id: sid,
                        finish_reason: FinishReason::TransportClosed,
                    },
                });
            }
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Closed,
            });
            {
                let mut s = shared.lock().unwrap();
                s.agent_state = AgentState::Closed;
            }
        });
        self.reader_handles.push(stdout_handle);

        // Stderr reader task
        let key_err = self.key.clone();
        let stderr_handle = tokio::spawn(async move {
            let stderr_async = match tokio::process::ChildStderr::from_std(stderr) {
                Ok(s) => s,
                Err(e) => {
                    warn!(key = %key_err, error = %e, "kimi: failed to convert stderr to async");
                    return;
                }
            };
            let reader = BufReader::new(stderr_async);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    warn!(key = %key_err, line = %line, "kimi stderr");
                }
            }
        });
        self.reader_handles.push(stderr_handle);

        self.child = Some(child);

        Ok(())
    }

    async fn new_session(&mut self) -> anyhow::Result<SessionId> {
        bail!("kimi does not support new_session on an active handle");
    }

    async fn resume_session(&mut self, _id: SessionId) -> anyhow::Result<()> {
        bail!("kimi does not support resume_session on an active handle");
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let session_id = {
            let s = self.shared.lock().unwrap();
            match &s.agent_state {
                AgentState::Active { session_id } => session_id.clone(),
                _ => bail!("cannot prompt: handle not in Active state"),
            }
        };

        let run_id = RunId::new_v4();
        let request_id = self.alloc_id();

        {
            let mut s = self.shared.lock().unwrap();
            s.run_id = Some(run_id);
            s.agent_state = AgentState::PromptInFlight {
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
            state: AgentState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            },
        });

        let prompt_req =
            acp_protocol::build_session_prompt_request(request_id, &session_id, &req.text);
        if let Some(ref tx) = self.stdin_tx {
            tx.send(prompt_req).await.context("stdin channel closed")?;
        } else {
            bail!("stdin not available — handle not started");
        }

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        // Read authoritative state from shared — self.state can lag the reader task.
        let (run_id, session_id) = {
            let s = self.shared.lock().unwrap();
            match &s.agent_state {
                AgentState::PromptInFlight { run_id, session_id } => (*run_id, session_id.clone()),
                _ => return Ok(CancelOutcome::NotInFlight),
            }
        };

        {
            let mut s = self.shared.lock().unwrap();
            s.run_id = None;
            s.agent_state = AgentState::Active {
                session_id: session_id.clone(),
            };
        }

        self.emit(DriverEvent::Completed {
            key: self.key.clone(),
            run_id,
            result: RunResult {
                session_id: session_id.clone(),
                finish_reason: FinishReason::Cancelled,
            },
        });

        self.state = AgentState::Active { session_id };
        Ok(CancelOutcome::Aborted)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.state, AgentState::Closed) {
            return Ok(());
        }

        if let Some(ref mut child) = self.child {
            let pid = child.id();
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        self.child = None;
        self.stdin_tx = None;

        self.state = AgentState::Closed;
        {
            let mut s = self.shared.lock().unwrap();
            s.agent_state = AgentState::Closed;
        }
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Closed,
        });
        self.events.close();

        for handle in self.reader_handles.drain(..) {
            handle.abort();
        }

        Ok(())
    }
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
            display_name: "test-kimi".to_string(),
            description: None,
            system_prompt: None,
            model: "kimi-code/kimi-for-coding".to_string(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_binary: String::new(),
            server_url: String::new(),
            bridge_endpoint: None,
        }
    }

    #[tokio::test]
    async fn test_kimi_driver_probe_not_installed() {
        let driver = KimiDriver;
        let probe = driver.probe().await.unwrap();
        // kimi binary is not on PATH in CI/test environments
        if probe.auth == ProbeAuth::NotInstalled {
            assert_eq!(probe.transport, TransportKind::AcpNative);
            assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
        }
    }

    #[tokio::test]
    async fn test_kimi_driver_list_models() {
        let driver = KimiDriver;
        let models = driver.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "kimi-code/kimi-for-coding");
    }

    #[tokio::test]
    async fn test_kimi_driver_attach_returns_idle() {
        let driver = KimiDriver;
        let result = driver
            .attach("kimi-agent-1".to_string(), test_spec())
            .await
            .unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }
}
