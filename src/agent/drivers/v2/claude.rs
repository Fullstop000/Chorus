//! v2 Claude driver backed by Claude headless CLI.

use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{bail, Context};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use super::claude_headless::{self, HeadlessEvent};
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
    pending_prompt: Option<String>,
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
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--input-format".into(), "stream-json".into(),
            "--output-format".into(), "stream-json".into(),
            "--verbose".into(),
            "--include-partial-messages".into(),
            "--permission-mode".into(), "acceptEdits".into(),
            "--allowedTools".into(),
            "Bash,Read,Edit,Write,MultiEdit,Glob,Grep,LS,mcp__chat__*".into(),
            "--mcp-config".into(), mcp_path_str,
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
        let stdin = std_child
            .stdin
            .take()
            .map(tokio::process::ChildStdin::from_std);

        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(64);

        if let Some(Ok(child_stdin)) = stdin {
            self.reader_handles
                .push(spawn_stdin_writer(child_stdin, stdin_rx));
        }

        // Prepare deferred prompt if provided
        let pending_prompt = init_prompt.map(|req| req.text);

        let shared = Arc::new(std::sync::Mutex::new(SharedReaderState {
            session_id: None,
            run_id: None,
            pending_prompt,
            agent_state: AgentState::Starting,
        }));
        self.shared = Some(shared.clone());

        self.reader_handles.push(spawn_stdout_reader(
            self.key.clone(),
            self.event_tx.clone(),
            stdout,
            shared,
            stdin_tx.clone(),
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

        if let Some(ref stdin_tx) = self.stdin_tx {
            stdin_tx
                .send(format!("{msg}\n"))
                .await
                .map_err(|e| anyhow::anyhow!("claude v2: stdin write failed: {e}"))?;
        }

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

fn spawn_stdout_reader(
    key: AgentKey,
    tx: mpsc::Sender<DriverEvent>,
    stdout: tokio::process::ChildStdout,
    shared: Arc<std::sync::Mutex<SharedReaderState>>,
    stdin_tx: mpsc::Sender<String>,
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
                    {
                        let mut guard = shared.lock().unwrap();
                        guard.session_id = Some(session_id.clone());
                        guard.agent_state = AgentState::Active {
                            session_id: session_id.clone(),
                        };
                    }

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

                        let msg = claude_headless::build_user_message(&prompt_text);
                        let _ = stdin_tx.send(format!("{msg}\n")).await;
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

                HeadlessEvent::ContentBlockStop { index } | HeadlessEvent::ToolUseStop { index } => {
                    // If index matches current tool accumulation, flush it
                    let should_flush =
                        pending_tool.as_ref().map_or(false, |t| t.3 == index);
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
}
