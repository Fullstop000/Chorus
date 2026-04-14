//! v2 Codex driver backed by the `codex-acp` ACP adapter binary.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use crate::agent::drivers::{command_exists, run_command};
use crate::agent::AgentRuntime;

use super::acp_protocol::{self, AcpParsed, AcpPhase, AcpUpdateItem, ToolCallAccumulator};
use super::*;

// ---------------------------------------------------------------------------
// CodexDriver — RuntimeDriver
// ---------------------------------------------------------------------------

pub struct CodexDriver;

#[async_trait]
impl RuntimeDriver for CodexDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Codex
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        if !command_exists("codex-acp") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::AcpAdapter,
                capabilities: CapabilitySet::MODEL_LIST,
            });
        }

        let auth = run_command("codex", &["login", "status"])
            .ok()
            .map(|result| {
                let combined = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
                if result.success && combined.contains("logged in") {
                    ProbeAuth::Authed
                } else {
                    ProbeAuth::Unauthed
                }
            })
            .unwrap_or(ProbeAuth::Unauthed);

        Ok(RuntimeProbe {
            auth,
            transport: TransportKind::AcpAdapter,
            capabilities: CapabilitySet::MODEL_LIST,
        })
    }

    async fn login(&self) -> anyhow::Result<LoginOutcome> {
        Ok(LoginOutcome::Failed {
            reason: "codex does not support login via Chorus".into(),
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo::from_id("gpt-5.4".into()),
            ModelInfo::from_id("gpt-5.4-mini".into()),
            ModelInfo::from_id("gpt-5.3-codex".into()),
            ModelInfo::from_id("gpt-5.2-codex".into()),
            ModelInfo::from_id("gpt-5.2".into()),
            ModelInfo::from_id("gpt-5.1-codex-max".into()),
            ModelInfo::from_id("gpt-5.1-codex-mini".into()),
        ])
    }

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(vec![])
    }

    async fn attach(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let (events, event_tx) = EventFanOut::new();
        let handle = CodexHandle {
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
}

// ---------------------------------------------------------------------------
// CodexHandle — AgentHandle
// ---------------------------------------------------------------------------

pub struct CodexHandle {
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

impl CodexHandle {
    fn emit(&self, event: DriverEvent) {
        let _ = self.event_tx.try_send(event);
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }
}

impl Drop for CodexHandle {
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

/// Codex requires a git repository in the working directory.
// fn pre_spawn_setup(working_directory: &std::path::Path) -> anyhow::Result<()> {
//     let git_dir = working_directory.join(".git");
//     if git_dir.exists() {
//         return Ok(());
//     }

//     Command::new("git")
//         .args(["init"])
//         .current_dir(working_directory)
//         .stdin(Stdio::null())
//         .stdout(Stdio::null())
//         .stderr(Stdio::null())
//         .status()
//         .context("codex: git init failed")?;

//     Command::new("git")
//         .args(["add", "-A"])
//         .current_dir(working_directory)
//         .stdin(Stdio::null())
//         .stdout(Stdio::null())
//         .stderr(Stdio::null())
//         .status()
//         .context("codex: git add failed")?;

//     let git_env = [
//         ("GIT_AUTHOR_NAME", "slock"),
//         ("GIT_AUTHOR_EMAIL", "slock@local"),
//         ("GIT_COMMITTER_NAME", "slock"),
//         ("GIT_COMMITTER_EMAIL", "slock@local"),
//     ];

//     Command::new("git")
//         .args(["commit", "--allow-empty", "-m", "init"])
//         .current_dir(working_directory)
//         .stdin(Stdio::null())
//         .stdout(Stdio::null())
//         .stderr(Stdio::null())
//         .envs(git_env)
//         .status()
//         .context("codex: git commit failed")?;

//     Ok(())
// }

#[async_trait]
impl AgentHandle for CodexHandle {
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
            bail!("codex: start called in non-idle state");
        }

        self.state = AgentState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        // pre_spawn_setup(&self.spec.working_directory)?;

        // Build CLI args: codex-acp -c key=value ...
        let mut args: Vec<String> = vec![
            "-c".into(),
            r#"approval_policy="never""#.into(),
            "-c".into(),
            r#"sandbox_mode="danger-full-access""#.into(),
        ];

        if let Some(ref effort) = self.spec.reasoning_effort {
            if let Ok(val) = serde_json::to_string(effort.as_str()) {
                args.push("-c".into());
                args.push(format!("model_reasoning_effort={val}"));
            }
        }

        if !self.spec.model.is_empty() {
            if let Ok(val) = serde_json::to_string(&self.spec.model) {
                args.push("-c".into());
                args.push(format!("model={val}"));
            }
        }

        let mut cmd = Command::new("codex-acp");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        for ev in &self.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut child = cmd.spawn().context("failed to spawn codex-acp")?;
        let stdout = child.stdout.take().context("codex: missing stdout")?;
        let stderr = child.stderr.take().context("codex: missing stderr")?;
        let mut stdin = child.stdin.take().context("codex: missing stdin")?;

        // Write handshake synchronously before handing stdin to async writer
        let init_req = acp_protocol::build_initialize_request(1);
        writeln!(stdin, "{init_req}").context("codex: failed to write initialize request")?;

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
        writeln!(stdin, "{session_req}").context("codex: failed to write session request")?;

        // Stash deferred initial prompt
        if let Some(ref req) = init_prompt {
            let mut shared = self.shared.lock().unwrap();
            shared.pending_prompt = Some(req.text.clone());
        }

        // Stdin writer task (spawn_blocking — std stdin is not async)
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
                trace!(line = %line, "codex stdout");
                let parsed = acp_protocol::parse_line(&line);

                match parsed {
                    AcpParsed::InitializeResponse => {
                        let mut s = shared.lock().unwrap();
                        s.phase = AcpPhase::AwaitingSessionResponse;
                        debug!("codex: initialize response received");
                    }

                    AcpParsed::SessionResponse { session_id } => {
                        let (sid, deferred_prompt) = {
                            let mut s = shared.lock().unwrap();
                            s.phase = AcpPhase::Active;
                            let sid = session_id
                                .or_else(|| s.pending_session_id.take())
                                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                            s.session_id = Some(sid.clone());
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
                        let option_id = acp_protocol::pick_best_option_id(&options);
                        debug!(
                            ?tool_name,
                            request_id, option_id, "codex: auto-approving permission"
                        );
                        let response =
                            acp_protocol::build_permission_response_raw(request_id, option_id);
                        let _ = stdin_tx_for_reader.try_send(response);
                    }

                    AcpParsed::Error { message } => {
                        warn!(message = %message, "codex: ACP error");
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
        });
        self.reader_handles.push(stdout_handle);

        // Stderr reader task
        let key_err = self.key.clone();
        let stderr_handle = tokio::spawn(async move {
            let reader = BufReader::new(tokio::process::ChildStderr::from_std(stderr).unwrap());
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    warn!(key = %key_err, line = %line, "codex stderr");
                }
            }
        });
        self.reader_handles.push(stderr_handle);

        self.child = Some(child);

        Ok(())
    }

    async fn new_session(&mut self) -> anyhow::Result<SessionId> {
        bail!("codex: new_session not supported on an active handle");
    }

    async fn resume_session(&mut self, _id: SessionId) -> anyhow::Result<()> {
        bail!("codex: resume_session not supported on an active handle");
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let session_id = match &self.state {
            AgentState::Active { session_id } => session_id.clone(),
            _ => bail!("codex: cannot prompt in non-active state"),
        };

        let run_id = RunId::new_v4();
        let request_id = self.alloc_id();

        {
            let mut s = self.shared.lock().unwrap();
            s.run_id = Some(run_id);
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
            tx.send(prompt_req)
                .await
                .context("codex: stdin channel closed")?;
        } else {
            bail!("codex: stdin not available — handle not started");
        }

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        if let AgentState::PromptInFlight { run_id, session_id } = &self.state {
            let run_id = *run_id;
            let session_id = session_id.clone();

            {
                let mut s = self.shared.lock().unwrap();
                s.run_id = None;
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
            display_name: "test-codex".into(),
            description: None,
            system_prompt: None,
            model: "gpt-5.4".into(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_binary: "bridge".into(),
            server_url: "http://localhost:3001".into(),
        }
    }

    #[tokio::test]
    async fn test_codex_driver_probe_not_installed() {
        let driver = CodexDriver;
        let probe = driver.probe().await.unwrap();
        assert_eq!(probe.transport, TransportKind::AcpAdapter);
        assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
    }

    #[tokio::test]
    async fn test_codex_driver_list_models() {
        let driver = CodexDriver;
        let models = driver.list_models().await.unwrap();
        assert_eq!(models.len(), 7);
        assert_eq!(models[0].id, "gpt-5.4");
        assert_eq!(models[1].id, "gpt-5.4-mini");
        assert_eq!(models[2].id, "gpt-5.3-codex");
        assert_eq!(models[3].id, "gpt-5.2-codex");
        assert_eq!(models[4].id, "gpt-5.2");
        assert_eq!(models[5].id, "gpt-5.1-codex-max");
        assert_eq!(models[6].id, "gpt-5.1-codex-mini");
    }

    #[tokio::test]
    async fn test_codex_driver_attach_returns_idle() {
        let driver = CodexDriver;
        let result = driver
            .attach("agent-codex-1".into(), test_spec())
            .await
            .unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }

    #[tokio::test]
    async fn test_codex_handle_initial_phase() {
        let driver = CodexDriver;
        let result = driver
            .attach("agent-codex-2".into(), test_spec())
            .await
            .unwrap();
        // Handle starts Idle
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }
}
