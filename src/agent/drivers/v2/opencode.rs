//! Native v2 driver for the OpenCode runtime using ACP protocol.

use anyhow::{bail, Context};
use async_trait::async_trait;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use crate::agent::drivers::{command_exists, run_command};
use crate::agent::AgentRuntime;

use super::acp_protocol::{self, AcpParsed, AcpPhase, AcpUpdateItem, ToolCallAccumulator};
use super::*;

// ---------------------------------------------------------------------------
// MCP config construction
// ---------------------------------------------------------------------------

/// Build the `mcp.chat` config block for `opencode.json`.
///
/// Produces the remote HTTP MCP shape, connecting the runtime to the shared
/// bridge at `{endpoint}/token/{token}/mcp`. Factored out so config-shape
/// tests don't need a live bridge.
fn build_mcp_chat_config(bridge_endpoint: &str, token: &str) -> serde_json::Value {
    let base = bridge_endpoint.trim_end_matches('/');
    serde_json::json!({
        "type": "remote",
        "url": format!("{base}/token/{token}/mcp"),
    })
}

// ---------------------------------------------------------------------------
// OpencodeDriver
// ---------------------------------------------------------------------------

pub struct OpencodeDriver;

fn parse_opencode_models(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[async_trait]
impl RuntimeDriver for OpencodeDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Opencode
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

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(vec![])
    }

    async fn attach(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let (events, event_tx) = EventFanOut::new();
        let handle = OpencodeHandle {
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
    pending_session_id: Option<String>,
    agent_state: AgentState,
}

// ---------------------------------------------------------------------------
// OpencodeHandle
// ---------------------------------------------------------------------------

pub struct OpencodeHandle {
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

impl OpencodeHandle {
    fn emit(&self, event: DriverEvent) {
        let _ = self.event_tx.try_send(event);
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }
}

impl Drop for OpencodeHandle {
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
impl AgentHandle for OpencodeHandle {
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
        self.state = AgentState::Starting;
        {
            let mut s = self.shared.lock().unwrap();
            s.agent_state = AgentState::Starting;
        }
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        // Build model ID: append reasoning effort as suffix if set
        let wd = &self.spec.working_directory;
        let model_id = match &self.spec.reasoning_effort {
            Some(variant) if !variant.is_empty() => {
                format!("{}/{}", self.spec.model, variant)
            }
            _ => self.spec.model.clone(),
        };

        // Pair with the shared HTTP bridge. If pairing fails we surface the
        // error — misconfiguration is loud.
        let endpoint = &self.spec.bridge_endpoint;
        let pairing_token = super::request_pairing_token(endpoint, &self.key)
            .await
            .with_context(|| {
                format!(
                    "failed to pair with bridge at {endpoint} for agent {}",
                    self.key
                )
            })?;

        // Write opencode.json to the working directory
        let config_path = wd.join("opencode.json");
        let mcp_chat = build_mcp_chat_config(endpoint, &pairing_token);
        let opencode_config = serde_json::json!({
            "model": model_id,
            "mcp": {
                "chat": mcp_chat,
            }
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&opencode_config)?,
        )
        .context("failed to write opencode.json")?;

        // Build CLI args
        let args = vec!["acp".to_string()];

        let mut cmd = Command::new("opencode");
        cmd.args(&args)
            .current_dir(&self.spec.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        for ev in &self.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut child = cmd.spawn().context("failed to spawn opencode")?;
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;
        let mut stdin = child.stdin.take().context("missing stdin")?;

        // Write handshake synchronously before handing stdin to the async writer
        let init_req = acp_protocol::build_initialize_request(1);
        writeln!(stdin, "{init_req}").context("failed to write initialize request")?;

        let session_new_params = serde_json::json!({
            "cwd": self.spec.working_directory,
            "mcpServers": []
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
            let stdout_async = match tokio::process::ChildStdout::from_std(stdout) {
                Ok(s) => s,
                Err(e) => {
                    warn!(key = %key, error = %e, "opencode: failed to convert stdout to async");
                    return;
                }
            };
            let reader = BufReader::new(stdout_async);
            let mut lines = reader.lines();
            let mut accumulator = ToolCallAccumulator::new();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                trace!(line = %line, "opencode stdout");
                let parsed = acp_protocol::parse_line(&line);

                match parsed {
                    AcpParsed::InitializeResponse => {
                        let mut s = shared.lock().unwrap();
                        s.phase = AcpPhase::AwaitingSessionResponse;
                        debug!("opencode: initialize response received");
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
                            request_id, option_id, "opencode: auto-approving permission"
                        );
                        let response =
                            acp_protocol::build_permission_response_raw(request_id, option_id);
                        let _ = stdin_tx_for_reader.try_send(response);
                    }

                    AcpParsed::Error { message } => {
                        warn!(message = %message, "opencode: ACP error");
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
            {
                let mut s = shared.lock().unwrap();
                s.agent_state = AgentState::Closed;
            }
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Closed,
            });
        });
        self.reader_handles.push(stdout_handle);

        // Stderr reader task
        let key_err = self.key.clone();
        let stderr_handle = tokio::spawn(async move {
            let stderr_async = match tokio::process::ChildStderr::from_std(stderr) {
                Ok(s) => s,
                Err(e) => {
                    warn!(key = %key_err, error = %e, "opencode: failed to convert stderr to async");
                    return;
                }
            };
            let reader = BufReader::new(stderr_async);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    warn!(key = %key_err, line = %line, "opencode stderr");
                }
            }
        });
        self.reader_handles.push(stderr_handle);

        self.child = Some(child);

        Ok(())
    }

    async fn new_session(&mut self) -> anyhow::Result<SessionId> {
        bail!("opencode does not support new_session on an active handle");
    }

    async fn resume_session(&mut self, _id: SessionId) -> anyhow::Result<()> {
        bail!("opencode does not support resume_session on an active handle");
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
        let cancel_info = {
            let s = self.shared.lock().unwrap();
            match &s.agent_state {
                AgentState::PromptInFlight { run_id, session_id } => {
                    Some((*run_id, session_id.clone()))
                }
                _ => None,
            }
        };
        if let Some((run_id, session_id)) = cancel_info {
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
        } else {
            Ok(CancelOutcome::NotInFlight)
        }
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
        let driver = OpencodeDriver;
        if !command_exists("opencode") {
            let models = driver.list_models().await.unwrap();
            assert!(models.is_empty());
        }
    }

    #[tokio::test]
    async fn test_opencode_driver_attach_returns_idle() {
        let driver = OpencodeDriver;
        let result = driver
            .attach("opencode-agent-1".to_string(), test_spec())
            .await
            .unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }

    #[test]
    fn build_mcp_chat_config_http_shape() {
        // Remote HTTP MCP shape — the only shape we produce.
        let config = build_mcp_chat_config("http://127.0.0.1:4321", "tok-xyz");
        assert_eq!(config["type"], "remote");
        assert_eq!(config["url"], "http://127.0.0.1:4321/token/tok-xyz/mcp");
        assert!(config.get("command").is_none());
    }

    #[test]
    fn build_mcp_chat_config_trims_trailing_slash() {
        // Endpoint with trailing slash must not produce `//token/` in the URL.
        let config = build_mcp_chat_config("http://127.0.0.1:4321/", "tok-xyz");
        assert_eq!(config["url"], "http://127.0.0.1:4321/token/tok-xyz/mcp");
    }
}
