//! v2 Codex driver backed by the `codex app-server` native protocol.
//!
//! Uses JSONL over stdio with the Codex app-server wire format, which omits
//! the `"jsonrpc":"2.0"` header present in ACP messages. See the companion
//! module [`super::codex_app_server`] for all message builders and parsers.

use std::collections::HashMap;
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

use super::codex_app_server::{self, AppServerEvent, AppServerPhase, ItemEvent, TurnStatus};
use super::*;

// ---------------------------------------------------------------------------
// MCP args construction
// ---------------------------------------------------------------------------

/// Build the `-c mcp_servers.chat.*` override flags for `codex app-server`.
///
/// Produces the native HTTP MCP shape, connecting the runtime to the shared
/// bridge at `{endpoint}/token/{token}/mcp`. Returns a flat `Vec<String>`
/// ready to be extended onto the args list; each config override is already
/// preceded by its own `-c` flag.
///
/// Factored out so config-shape tests don't need a live bridge.
fn build_codex_mcp_args(bridge_endpoint: &str, token: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    let url = crate::bridge::token_mcp_url(bridge_endpoint, token);
    let url_json = serde_json::to_string(&url).expect("url serialization cannot fail");
    args.push("-c".into());
    args.push(format!("mcp_servers.chat.url={url_json}"));
    args.push("-c".into());
    args.push("mcp_servers.chat.enabled=true".into());
    args.push("-c".into());
    args.push("mcp_servers.chat.required=true".into());

    args
}

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
        if !command_exists("codex") {
            return Ok(RuntimeProbe {
                auth: ProbeAuth::NotInstalled,
                transport: TransportKind::CodexAppServer,
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
            transport: TransportKind::CodexAppServer,
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
            shared: None,
            next_request_id: 2, // 0 = initialize, 1 = thread, 2+ = turns
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

/// State shared between the `CodexHandle` and the stdout reader task.
struct SharedReaderState {
    phase: AppServerPhase,
    thread_id: Option<String>,
    turn_id: Option<String>,
    run_id: Option<RunId>,
    pending_prompt: Option<String>,
    /// Set when start() is called with a resume_session_id.
    pending_thread_id: Option<String>,
    /// Authoritative agent state — read by `state()` on the handle.
    agent_state: AgentState,
    /// Per-item command output buffer, keyed by item_id, capped at 256 KB each.
    cmd_output_buf: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// CodexHandle — AgentHandle
// ---------------------------------------------------------------------------

pub struct CodexHandle {
    key: AgentKey,
    /// Pre-start state only; after start() consult shared.agent_state instead.
    state: AgentState,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    spec: AgentSpec,
    child: Option<std::process::Child>,
    stdin_tx: Option<mpsc::Sender<String>>,
    shared: Option<Arc<Mutex<SharedReaderState>>>,
    next_request_id: u64,
    reader_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl CodexHandle {
    fn emit(&self, event: DriverEvent) {
        if let Err(e) = self.event_tx.try_send(event) {
            warn!("codex v2: failed to emit event: {e}");
        }
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

#[async_trait]
impl AgentHandle for CodexHandle {
    fn key(&self) -> &AgentKey {
        &self.key
    }

    /// Returns the authoritative agent state.
    ///
    /// Once `start()` has been called, reads from `shared.agent_state` so the
    /// value reflects transitions made by the async reader task.
    fn state(&self) -> AgentState {
        if let Some(ref shared) = self.shared {
            shared.lock().unwrap().agent_state.clone()
        } else {
            self.state.clone()
        }
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

        // Build CLI args: `codex app-server [flags]`
        // NOTE: codex app-server only accepts `-c key=value` overrides plus
        // transport flags. Model, cwd, approvalPolicy, and sandbox are all
        // passed in the `thread/start` JSON-RPC params instead.
        let mut args: Vec<String> = vec!["app-server".into()];

        // Obtain a pairing token from the shared HTTP bridge.
        let token = super::request_pairing_token(&self.spec.bridge_endpoint, &self.key)
            .await
            .context("failed to pair with shared bridge")?;

        // Register the bridge MCP server so Codex can call back into Chorus.
        // Uses the same `mcp_servers.*` config key format as ~/.codex/config.toml.
        let mcp_args = build_codex_mcp_args(&self.spec.bridge_endpoint, &token);
        args.extend(mcp_args);

        let mut cmd = Command::new("codex");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        for ev in &self.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut child = cmd.spawn().context("failed to spawn codex")?;
        let stdout_raw = child.stdout.take().context("codex: missing stdout")?;
        let stderr_raw = child.stderr.take().context("codex: missing stderr")?;
        let mut stdin = child.stdin.take().context("codex: missing stdin")?;

        // Convert to async before spawning so any error propagates here under ?
        let stdout_async =
            tokio::process::ChildStdout::from_std(stdout_raw).context("codex: convert stdout")?;
        let stderr_async =
            tokio::process::ChildStderr::from_std(stderr_raw).context("codex: convert stderr")?;

        // Write the initialize request synchronously before handing stdin off
        let init_req = codex_app_server::build_initialize(0);
        writeln!(stdin, "{init_req}").context("codex: failed to write initialize request")?;

        // Stash state that the reader task will need
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: AppServerPhase::AwaitingInitResponse,
            thread_id: None,
            turn_id: None,
            run_id: None,
            pending_prompt: init_prompt.map(|p| p.text),
            pending_thread_id: opts.resume_session_id.clone(),
            agent_state: AgentState::Starting,
            cmd_output_buf: HashMap::new(),
        }));
        self.shared = Some(shared.clone());

        // The reader task will issue build_turn_start(2, ...) for any deferred
        // initial prompt. Advance next_request_id past 2 so subsequent prompt()
        // calls (which use alloc_id()) don't collide with that hardcoded id.
        if shared.lock().unwrap().pending_prompt.is_some() {
            self.next_request_id = 3;
        }

        // Stdin writer task (spawn_blocking because std::io::Stdin is not async)
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
        self.stdin_tx = Some(stdin_tx);
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

        // Capture fields needed by the reader task
        let key = self.key.clone();
        let event_tx = self.event_tx.clone();
        let stdin_tx_reader = self.stdin_tx.clone().unwrap();
        let model_str = self.spec.model.clone();
        let cwd_str = self.spec.working_directory.to_string_lossy().into_owned();
        let system_prompt_str = self.spec.system_prompt.clone();

        // Stdout reader task
        let stdout_handle = tokio::spawn(async move {
            let reader = BufReader::new(stdout_async);
            let mut lines = reader.lines();

            // Local helper: try_send with drop logging (emit() on self is not accessible here)
            let emit = {
                let tx = event_tx.clone();
                move |ev: DriverEvent| {
                    if let Err(e) = tx.try_send(ev) {
                        warn!("codex v2: dropped event in reader: {e}");
                    }
                }
            };

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                trace!(line = %line, "codex stdout");
                let parsed = codex_app_server::parse_line(&line);

                match parsed {
                    AppServerEvent::InitializeResponse => {
                        // Transition phase and build both messages before releasing lock
                        let (init_notif, thread_req) = {
                            let mut s = shared.lock().unwrap();
                            s.phase = AppServerPhase::AwaitingThreadResponse;
                            let notif = codex_app_server::build_initialized();
                            let req = if let Some(ref tid) = s.pending_thread_id {
                                codex_app_server::build_thread_resume(1, tid)
                            } else {
                                codex_app_server::build_thread_start(
                                    1,
                                    &model_str,
                                    &cwd_str,
                                    system_prompt_str.as_deref(),
                                )
                            };
                            (notif, req)
                        };
                        let _ = stdin_tx_reader.try_send(init_notif);
                        let _ = stdin_tx_reader.try_send(thread_req);
                        debug!("codex: initialize response received; sent initialized + thread request");
                    }

                    AppServerEvent::ThreadResponse { thread_id } => {
                        let (pending_prompt, initial_run_id) = {
                            let mut s = shared.lock().unwrap();
                            s.phase = AppServerPhase::Active;
                            s.thread_id = Some(thread_id.clone());
                            s.agent_state = AgentState::Active {
                                session_id: thread_id.clone(),
                            };
                            let prompt = s.pending_prompt.take();
                            let run_id = if prompt.is_some() {
                                let rid = RunId::new_v4();
                                s.run_id = Some(rid);
                                s.agent_state = AgentState::PromptInFlight {
                                    run_id: rid,
                                    session_id: thread_id.clone(),
                                };
                                Some(rid)
                            } else {
                                None
                            };
                            (prompt, run_id)
                        };

                        // Emit events outside the lock
                        emit(DriverEvent::SessionAttached {
                            key: key.clone(),
                            session_id: thread_id.clone(),
                        });
                        emit(DriverEvent::Lifecycle {
                            key: key.clone(),
                            state: AgentState::Active {
                                session_id: thread_id.clone(),
                            },
                        });

                        if let (Some(prompt_text), Some(run_id)) = (pending_prompt, initial_run_id)
                        {
                            emit(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::PromptInFlight {
                                    run_id,
                                    session_id: thread_id.clone(),
                                },
                            });
                            // id=2: the first turn; alloc_id starts at 2 on the handle side
                            // but the reader uses fixed 2 since turns beyond the deferred
                            // initial one are tracked by prompt() using alloc_id
                            let turn_req =
                                codex_app_server::build_turn_start(2, &thread_id, &prompt_text);
                            let _ = stdin_tx_reader.try_send(turn_req);
                        }
                        debug!("codex: thread active; session_id = {}", thread_id);
                    }

                    AppServerEvent::TurnResponse { turn_id } => {
                        {
                            let mut s = shared.lock().unwrap();
                            s.turn_id = Some(turn_id.clone());
                        }
                        debug!("codex: turn started; turn_id = {}", turn_id);
                    }

                    AppServerEvent::TurnInterruptResponse => {
                        debug!("codex: turn interrupt acknowledged");
                    }

                    AppServerEvent::TurnCompleted { turn_id: _, status } => {
                        let (run_id, thread_id) = {
                            let mut s = shared.lock().unwrap();
                            let rid = s.run_id.take();
                            s.turn_id = None;
                            // Drain accumulated command output to prevent unbounded growth
                            s.cmd_output_buf.clear();
                            let tid = s.thread_id.clone().unwrap_or_default();
                            if rid.is_some() {
                                s.agent_state = AgentState::Active {
                                    session_id: tid.clone(),
                                };
                            }
                            (rid, tid)
                        };
                        if let Some(run_id) = run_id {
                            let finish_reason = match &status {
                                TurnStatus::Completed => FinishReason::Natural,
                                TurnStatus::Interrupted => FinishReason::Cancelled,
                                // Emit the error text so it appears in the channel.
                                TurnStatus::Failed { message } => {
                                    emit(DriverEvent::Output {
                                        key: key.clone(),
                                        run_id,
                                        item: AgentEventItem::Text {
                                            text: format!("⚠️ {message}"),
                                        },
                                    });
                                    FinishReason::Natural
                                }
                            };
                            emit(DriverEvent::Output {
                                key: key.clone(),
                                run_id,
                                item: AgentEventItem::TurnEnd,
                            });
                            emit(DriverEvent::Completed {
                                key: key.clone(),
                                run_id,
                                result: RunResult {
                                    session_id: thread_id.clone(),
                                    finish_reason,
                                },
                            });
                            emit(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::Active {
                                    session_id: thread_id,
                                },
                            });
                        }
                    }

                    AppServerEvent::AgentMessageDelta { item_id: _, text } => {
                        let run_id = { shared.lock().unwrap().run_id };
                        if let Some(run_id) = run_id {
                            emit(DriverEvent::Output {
                                key: key.clone(),
                                run_id,
                                item: AgentEventItem::Text { text },
                            });
                        }
                    }

                    AppServerEvent::ReasoningSummaryDelta { item_id: _, text } => {
                        let run_id = { shared.lock().unwrap().run_id };
                        if let Some(run_id) = run_id {
                            emit(DriverEvent::Output {
                                key: key.clone(),
                                run_id,
                                item: AgentEventItem::Thinking { text },
                            });
                        }
                    }

                    AppServerEvent::CommandOutputDelta { item_id, text } => {
                        // Buffer up to 256 KB per command item; still forward each delta.
                        // Drained at TurnCompleted to avoid unbounded growth.
                        const MAX_BUF: usize = 256 * 1024;
                        let run_id = {
                            let mut s = shared.lock().unwrap();
                            let buf = s.cmd_output_buf.entry(item_id.clone()).or_default();
                            if buf.len() + text.len() <= MAX_BUF {
                                buf.push_str(&text);
                            }
                            s.run_id
                        };
                        if let Some(run_id) = run_id {
                            emit(DriverEvent::Output {
                                key: key.clone(),
                                run_id,
                                item: AgentEventItem::Text { text },
                            });
                        }
                    }

                    AppServerEvent::CommandApproval { request_id, .. } => {
                        // approval_policy=never should prevent these; approve defensively if received
                        let resp = codex_app_server::build_approval_response(&request_id, "accept");
                        let _ = stdin_tx_reader.try_send(resp);
                        debug!("codex: auto-approved command execution");
                    }

                    AppServerEvent::FileChangeApproval { request_id, .. } => {
                        let resp = codex_app_server::build_approval_response(&request_id, "accept");
                        let _ = stdin_tx_reader.try_send(resp);
                        debug!("codex: auto-approved file change");
                    }

                    AppServerEvent::Error { message, .. } => {
                        warn!(message = %message, "codex: protocol error");
                        let run_id = {
                            let mut s = shared.lock().unwrap();
                            s.run_id.take()
                        };
                        if let Some(run_id) = run_id {
                            emit(DriverEvent::Failed {
                                key: key.clone(),
                                run_id,
                                error: AgentError::RuntimeReported(message),
                            });
                        }
                    }

                    // ItemCompleted: emit ToolCall/ToolResult trace events so the
                    // Telescope can show what tools the agent used.
                    AppServerEvent::ItemCompleted { item } => {
                        let run_id = { shared.lock().unwrap().run_id };
                        if let Some(run_id) = run_id {
                            match item {
                                ItemEvent::CommandExecution {
                                    id,
                                    command,
                                    exit_code,
                                    ..
                                } => {
                                    let output = {
                                        let s = shared.lock().unwrap();
                                        s.cmd_output_buf.get(&id).cloned().unwrap_or_default()
                                    };
                                    emit(DriverEvent::Output {
                                        key: key.clone(),
                                        run_id,
                                        item: AgentEventItem::ToolCall {
                                            name: "shell".to_string(),
                                            input: serde_json::json!({ "command": command }),
                                        },
                                    });
                                    let result = match exit_code {
                                        Some(code) if !output.is_empty() => {
                                            format!("(exit {code}) {output}")
                                        }
                                        Some(code) => format!("exit_code={code}"),
                                        None => output,
                                    };
                                    emit(DriverEvent::Output {
                                        key: key.clone(),
                                        run_id,
                                        item: AgentEventItem::ToolResult { content: result },
                                    });
                                }
                                ItemEvent::McpToolCall {
                                    server,
                                    tool,
                                    arguments,
                                    ..
                                } => {
                                    emit(DriverEvent::Output {
                                        key: key.clone(),
                                        run_id,
                                        item: AgentEventItem::ToolCall {
                                            name: format!("{server}/{tool}"),
                                            input: arguments,
                                        },
                                    });
                                }
                                _ => {}
                            }
                        }
                    }

                    // Informational notifications — no action required
                    AppServerEvent::ThreadStarted { .. }
                    | AppServerEvent::TurnStarted { .. }
                    | AppServerEvent::ItemStarted { .. }
                    | AppServerEvent::Unknown => {}
                }
            }

            // EOF — `codex` process exited
            let (run_id, session_id) = {
                let s = shared.lock().unwrap();
                (s.run_id, s.thread_id.clone().unwrap_or_default())
            };
            if let Some(run_id) = run_id {
                emit(DriverEvent::Completed {
                    key: key.clone(),
                    run_id,
                    result: RunResult {
                        session_id,
                        finish_reason: FinishReason::TransportClosed,
                    },
                });
            }
            emit(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Closed,
            });
        });
        self.reader_handles.push(stdout_handle);

        // Stderr reader task — just log for diagnostics
        let key_err = self.key.clone();
        let stderr_handle = tokio::spawn(async move {
            let reader = BufReader::new(stderr_async);
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
        // Read session_id from shared state — self.state may lag the reader task
        let session_id = if let Some(ref shared) = self.shared {
            let s = shared.lock().unwrap();
            match &s.agent_state {
                AgentState::Active { session_id } => session_id.clone(),
                _ => bail!("codex: cannot prompt in non-active state"),
            }
        } else {
            bail!("codex: cannot prompt — handle not started");
        };

        let run_id = RunId::new_v4();
        let request_id = self.alloc_id();

        {
            let mut s = self.shared.as_ref().unwrap().lock().unwrap();
            s.run_id = Some(run_id);
            s.agent_state = AgentState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            };
        }

        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            },
        });

        let turn_req = codex_app_server::build_turn_start(request_id, &session_id, &req.text);
        if let Some(ref tx) = self.stdin_tx {
            tx.send(turn_req)
                .await
                .context("codex: stdin channel closed")?;
        } else {
            bail!("codex: stdin not available — handle not started");
        }

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        let is_in_flight = if let Some(ref shared) = self.shared {
            matches!(
                shared.lock().unwrap().agent_state,
                AgentState::PromptInFlight { .. }
            )
        } else {
            false
        };

        if !is_in_flight {
            return Ok(CancelOutcome::NotInFlight);
        }

        let (run_id, session_id, thread_id, turn_id) = {
            let mut s = self.shared.as_ref().unwrap().lock().unwrap();
            let run_id = s.run_id.take();
            let session_id = s.thread_id.clone().unwrap_or_default();
            let thread_id = s.thread_id.clone().unwrap_or_default();
            let turn_id = s.turn_id.take();
            s.agent_state = AgentState::Active {
                session_id: session_id.clone(),
            };
            (run_id, session_id, thread_id, turn_id)
        };

        // Send a real turn/interrupt if we have enough context
        if !thread_id.is_empty() {
            if let (Some(vid), Some(tx)) = (turn_id, self.stdin_tx.clone()) {
                let req_id = self.alloc_id();
                let interrupt = codex_app_server::build_turn_interrupt(req_id, &thread_id, &vid);
                let _ = tx.try_send(interrupt);
            }
        }

        // Emit synthetic completion so callers aren't left waiting
        if let Some(run_id) = run_id {
            self.emit(DriverEvent::Completed {
                key: self.key.clone(),
                run_id,
                result: RunResult {
                    session_id,
                    finish_reason: FinishReason::Cancelled,
                },
            });
        }

        Ok(CancelOutcome::Aborted)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.state(), AgentState::Closed) {
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

        if let Some(ref shared) = self.shared {
            shared.lock().unwrap().agent_state = AgentState::Closed;
        }
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
            bridge_endpoint: "http://127.0.0.1:1".into(),
        }
    }

    #[tokio::test]
    async fn test_codex_driver_probe_not_installed() {
        let driver = CodexDriver;
        let probe = driver.probe().await.unwrap();
        // When the codex binary is absent the transport must be CodexAppServer
        assert_eq!(probe.transport, TransportKind::CodexAppServer);
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
    async fn test_codex_handle_shared_is_none_before_start() {
        // Before start(), state() falls back to self.state which is Idle.
        // Verifies attach() leaves shared=None and state() falls back correctly.
        let driver = CodexDriver;
        let result = driver
            .attach("agent-codex-3".into(), test_spec())
            .await
            .unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }

    // ---- build_codex_mcp_args tests ----

    #[test]
    fn build_codex_mcp_args_http_shape() {
        // Native HTTP MCP url shape — the only shape we produce.
        let args = build_codex_mcp_args("http://127.0.0.1:4321", "tok-xyz");

        let joined = args.join(" ");
        assert!(
            joined.contains("mcp_servers.chat.url="),
            "expected url override, got: {joined}"
        );
        assert!(
            joined.contains("tok-xyz"),
            "expected token in url, got: {joined}"
        );
        assert!(
            !joined.contains("mcp_servers.chat.command="),
            "unexpected command override in http path: {joined}"
        );
        assert!(
            !joined.contains("mcp_servers.chat.args="),
            "unexpected args override in http path: {joined}"
        );
        assert!(
            !joined.contains("mcp_servers.chat.type="),
            "unexpected type override in http path (url implies transport): {joined}"
        );
        assert!(joined.contains("mcp_servers.chat.enabled=true"));
        assert!(joined.contains("mcp_servers.chat.required=true"));
        // Each config value must be preceded by -c
        for i in (0..args.len()).step_by(2) {
            assert_eq!(args[i], "-c", "expected -c at index {i}, got: {}", args[i]);
        }
        // Verify the URL value itself (it's JSON-encoded in the arg)
        let url_arg = args
            .iter()
            .find(|a| a.starts_with("mcp_servers.chat.url="))
            .expect("url arg not found");
        let json_val = url_arg.trim_start_matches("mcp_servers.chat.url=");
        let decoded: String =
            serde_json::from_str(json_val).expect("url value should be JSON string");
        assert_eq!(decoded, "http://127.0.0.1:4321/token/tok-xyz/mcp");
    }

    #[test]
    fn build_codex_mcp_args_trims_trailing_slash() {
        // Endpoint with trailing slash must not produce `//token/` in the URL.
        let args = build_codex_mcp_args("http://127.0.0.1:4321/", "tok-xyz");

        let url_arg = args
            .iter()
            .find(|a| a.starts_with("mcp_servers.chat.url="))
            .expect("url arg not found");
        let json_val = url_arg.trim_start_matches("mcp_servers.chat.url=");
        let decoded: String =
            serde_json::from_str(json_val).expect("url value should be JSON string");
        assert_eq!(decoded, "http://127.0.0.1:4321/token/tok-xyz/mcp");
        assert!(
            !decoded.contains("//token/"),
            "double-slash in URL: {decoded}"
        );
    }
}
