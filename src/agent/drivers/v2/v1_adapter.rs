//! Adapter that wraps a v1 [`Driver`] behind the v2 [`RuntimeDriver`] +
//! [`AgentHandle`] traits, allowing incremental migration without rewriting
//! every runtime at once.

use std::sync::Arc;

use anyhow::bail;
use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::agent::config::AgentConfig;
use crate::agent::drivers::{Driver, ParsedEvent, SpawnContext};
use crate::agent::runtime_status::RuntimeAuthStatus;
use crate::agent::AgentRuntime;

use super::{
    AgentError, AgentEventItem, AgentHandle, AgentKey, AgentSpec, AgentState, AttachResult,
    CancelOutcome, CapabilitySet, DriverEvent, EventFanOut, EventStreamHandle, FinishReason,
    LoginOutcome, ModelInfo, ProbeAuth, PromptReq, RunId, RunResult, RuntimeDriver, RuntimeProbe,
    SessionId, SlashCommand, StartOpts, StoredSessionMeta, TransportKind,
};

// ---------------------------------------------------------------------------
// V1DriverAdapter — RuntimeDriver
// ---------------------------------------------------------------------------

/// Wraps a v1 [`Driver`] as a v2 [`RuntimeDriver`].
pub struct V1DriverAdapter {
    inner: Arc<dyn Driver>,
    runtime: AgentRuntime,
}

impl V1DriverAdapter {
    pub fn new(inner: Arc<dyn Driver>) -> Self {
        let runtime = inner.runtime();
        Self { inner, runtime }
    }
}

#[async_trait]
impl RuntimeDriver for V1DriverAdapter {
    fn runtime(&self) -> AgentRuntime {
        self.runtime
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        let status = self.inner.detect_runtime_status()?;
        let auth = if !status.installed {
            ProbeAuth::NotInstalled
        } else if status.auth_status == Some(RuntimeAuthStatus::Authed) {
            ProbeAuth::Authed
        } else {
            ProbeAuth::Unauthed
        };
        Ok(RuntimeProbe {
            auth,
            transport: TransportKind::AcpAdapter,
            capabilities: CapabilitySet::MODEL_LIST,
        })
    }

    async fn login(&self) -> anyhow::Result<LoginOutcome> {
        bail!("v1 adapter does not support login")
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(Vec::new())
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let ids = self.inner.list_models()?;
        Ok(ids.into_iter().map(ModelInfo::from_id).collect())
    }

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(Vec::new())
    }

    async fn attach(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let (handle, events) = V1HandleAdapter::new(self.inner.clone(), self.runtime, key, spec);
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }
}

// ---------------------------------------------------------------------------
// Shared state between handle and stdout reader task
// ---------------------------------------------------------------------------

/// Mutable state shared between the [`V1HandleAdapter`] and its stdout reader
/// task via `Arc<std::sync::Mutex<_>>`.
struct SharedReaderState {
    run_id: Option<RunId>,
    session_id: Option<SessionId>,
}

// ---------------------------------------------------------------------------
// V1HandleAdapter — AgentHandle
// ---------------------------------------------------------------------------

/// Wraps a v1 [`Driver`] as a v2 [`AgentHandle`].
pub struct V1HandleAdapter {
    inner: Arc<dyn Driver>,
    runtime: AgentRuntime,
    key: AgentKey,
    spec: AgentSpec,
    state: AgentState,
    child: Option<std::process::Child>,
    child_pid: Option<u32>,
    #[allow(dead_code)]
    session_id: Option<SessionId>,
    /// Channel for writing to the child's stdin from prompt()/WriteStdin.
    stdin_tx: Option<mpsc::Sender<String>>,
    inbound_tx: mpsc::Sender<DriverEvent>,
    events: EventStreamHandle,
    #[allow(dead_code)]
    current_run_id: Option<RunId>,
    shared: Arc<std::sync::Mutex<SharedReaderState>>,
}

impl V1HandleAdapter {
    pub fn new(
        inner: Arc<dyn Driver>,
        runtime: AgentRuntime,
        key: AgentKey,
        spec: AgentSpec,
    ) -> (Self, EventStreamHandle) {
        let (events, inbound_tx) = EventFanOut::new();
        let shared = Arc::new(std::sync::Mutex::new(SharedReaderState {
            run_id: None,
            session_id: None,
        }));
        let handle = Self {
            inner,
            runtime,
            key,
            spec,
            state: AgentState::Idle,
            child: None,
            child_pid: None,
            session_id: None,
            stdin_tx: None,
            inbound_tx,
            events: events.clone(),
            current_run_id: None,
            shared,
        };
        (handle, events)
    }

    /// Emit a driver event, logging failures rather than propagating them.
    fn emit(&self, event: DriverEvent) {
        if let Err(e) = self.inbound_tx.try_send(event) {
            tracing::warn!("v1 adapter: failed to emit event: {e}");
        }
    }
}

#[async_trait]
impl AgentHandle for V1HandleAdapter {
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
            bail!("v1 adapter: start called in non-idle state");
        }

        self.state = AgentState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        let config = AgentConfig {
            name: self.key.clone(),
            display_name: self.spec.display_name.clone(),
            description: self.spec.description.clone(),
            system_prompt: self.spec.system_prompt.clone(),
            runtime: self.runtime.as_str().to_string(),
            model: self.spec.model.clone(),
            session_id: opts.resume_session_id.clone(),
            reasoning_effort: self.spec.reasoning_effort.clone(),
            env_vars: self.spec.env_vars.clone(),
        };
        let ctx = SpawnContext {
            agent_id: self.key.clone(),
            agent_name: self.key.clone(),
            config,
            prompt: String::new(),
            working_directory: self.spec.working_directory.to_string_lossy().to_string(),
            bridge_binary: self.spec.bridge_binary.clone(),
            server_url: self.spec.server_url.clone(),
        };

        let mut std_child = self.inner.spawn(&ctx)?;
        self.child_pid = Some(std_child.id());

        // Take I/O handles from the std child and convert to tokio async types.
        let stdout = std_child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("v1 adapter: child has no stdout"))?;
        let stdout = tokio::process::ChildStdout::from_std(stdout)?;

        let stderr = std_child
            .stderr
            .take()
            .map(tokio::process::ChildStderr::from_std);

        let stdin = std_child
            .stdin
            .take()
            .map(tokio::process::ChildStdin::from_std);

        // Channel for stdin writes originating from ParsedEvent::WriteStdin.
        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(64);

        // Spawn stdin writer task.
        if let Some(Ok(child_stdin)) = stdin {
            spawn_stdin_writer(child_stdin, stdin_rx);
        }

        spawn_stdout_reader(
            self.inner.clone(),
            self.key.clone(),
            self.inbound_tx.clone(),
            stdout,
            self.shared.clone(),
            stdin_tx.clone(),
        );

        if let Some(Ok(child_stderr)) = stderr {
            spawn_stderr_reader(child_stderr);
        }

        self.child = Some(std_child);
        self.stdin_tx = Some(stdin_tx);

        if let Some(req) = init_prompt {
            self.prompt(req).await?;
        } else {
            let session_id = String::new();
            self.state = AgentState::Active {
                session_id: session_id.clone(),
            };
            self.emit(DriverEvent::Lifecycle {
                key: self.key.clone(),
                state: AgentState::Active { session_id },
            });
        }

        Ok(())
    }

    async fn new_session(&mut self) -> anyhow::Result<SessionId> {
        bail!("v1 adapter does not support new_session")
    }

    async fn resume_session(&mut self, _id: SessionId) -> anyhow::Result<()> {
        bail!("v1 adapter does not support resume_session")
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let run_id = RunId::new_v4();
        let session_id = {
            let guard = self.shared.lock().unwrap();
            guard.session_id.clone().unwrap_or_default()
        };

        let encoded = self
            .inner
            .encode_stdin_message(&req.text, &session_id)
            .ok_or_else(|| {
                anyhow::anyhow!("v1 adapter: runtime does not support stdin messages")
            })?;

        // Write encoded message to child stdin via the channel.
        if let Some(ref stdin_tx) = self.stdin_tx {
            stdin_tx
                .send(format!("{encoded}\n"))
                .await
                .map_err(|e| anyhow::anyhow!("v1 adapter: stdin write failed: {e}"))?;
        }

        // Update shared run_id so reader task can correlate events.
        {
            let mut guard = self.shared.lock().unwrap();
            guard.run_id = Some(run_id);
        }
        self.current_run_id = Some(run_id);

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

        self.state = AgentState::Closed;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Closed,
        });
        self.events.close();
        Ok(())
    }
}

impl Drop for V1HandleAdapter {
    fn drop(&mut self) {
        if let Some(pid) = self.child_pid {
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
    inner: Arc<dyn Driver>,
    key: AgentKey,
    tx: mpsc::Sender<DriverEvent>,
    stdout: tokio::process::ChildStdout,
    shared: Arc<std::sync::Mutex<SharedReaderState>>,
    stdin_tx: mpsc::Sender<String>,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut last_tool_name: Option<String> = None;

        while let Ok(Some(line)) = lines.next_line().await {
            let events = inner.parse_line(&line);
            for parsed in events {
                let (run_id, session_id) = {
                    let guard = shared.lock().unwrap();
                    (guard.run_id, guard.session_id.clone())
                };

                match parsed {
                    ParsedEvent::SessionInit { session_id: sid } => {
                        {
                            let mut guard = shared.lock().unwrap();
                            guard.session_id = Some(sid.clone());
                        }
                        let _ = tx
                            .send(DriverEvent::SessionAttached {
                                key: key.clone(),
                                session_id: sid.clone(),
                            })
                            .await;
                        let _ = tx
                            .send(DriverEvent::Lifecycle {
                                key: key.clone(),
                                state: AgentState::Active { session_id: sid },
                            })
                            .await;
                    }
                    ParsedEvent::Thinking { text } => {
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
                    ParsedEvent::Text { text } => {
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
                    ParsedEvent::ToolCall { name, input } => {
                        last_tool_name = Some(name.clone());
                        if let Some(rid) = run_id {
                            let _ = tx
                                .send(DriverEvent::Output {
                                    key: key.clone(),
                                    run_id: rid,
                                    item: AgentEventItem::ToolCall { name, input },
                                })
                                .await;
                        }
                    }
                    ParsedEvent::ToolCallUpdate { input } => {
                        if let Some(rid) = run_id {
                            let _ = tx
                                .send(DriverEvent::Output {
                                    key: key.clone(),
                                    run_id: rid,
                                    item: AgentEventItem::ToolCall {
                                        name: last_tool_name.clone().unwrap_or_default(),
                                        input,
                                    },
                                })
                                .await;
                        }
                    }
                    ParsedEvent::ToolResult { content } => {
                        if let Some(rid) = run_id {
                            let _ = tx
                                .send(DriverEvent::Output {
                                    key: key.clone(),
                                    run_id: rid,
                                    item: AgentEventItem::ToolResult { content },
                                })
                                .await;
                        }
                    }
                    ParsedEvent::TurnEnd {
                        session_id: turn_sid,
                    } => {
                        if let Some(rid) = run_id {
                            let resolved_sid = turn_sid.or(session_id).unwrap_or_default();
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
                                        session_id: resolved_sid,
                                        finish_reason: FinishReason::Natural,
                                    },
                                })
                                .await;
                        }
                    }
                    ParsedEvent::Error { message } => {
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
                    ParsedEvent::WriteStdin { data } => {
                        let _ = stdin_tx.send(data).await;
                    }
                    ParsedEvent::PermissionRequested { .. } => {
                        // Auto-approve in v1 — no-op.
                    }
                }
            }
        }

        // stdout EOF — emit Closed lifecycle if not already.
        let _ = tx
            .send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Closed,
            })
            .await;
    });
}

fn spawn_stderr_reader(stderr: tokio::process::ChildStderr) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(target: "v1_adapter_stderr", "{}", line);
        }
    });
}

fn spawn_stdin_writer(mut stdin: tokio::process::ChildStdin, mut rx: mpsc::Receiver<String>) {
    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if let Err(e) = stdin.write_all(data.as_bytes()).await {
                tracing::warn!("v1 adapter: stdin write failed: {e}");
                break;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime_status::RuntimeStatus;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};

    /// Minimal v1 Driver test double.
    struct FakeV1Driver {
        runtime: AgentRuntime,
        status: RuntimeStatus,
        models: Vec<String>,
    }

    impl FakeV1Driver {
        fn authed(models: Vec<String>) -> Self {
            Self {
                runtime: AgentRuntime::Claude,
                status: RuntimeStatus {
                    runtime: "claude".into(),
                    installed: true,
                    auth_status: Some(RuntimeAuthStatus::Authed),
                },
                models,
            }
        }

        fn unauthed() -> Self {
            Self {
                runtime: AgentRuntime::Claude,
                status: RuntimeStatus {
                    runtime: "claude".into(),
                    installed: true,
                    auth_status: None,
                },
                models: vec![],
            }
        }

        fn not_installed() -> Self {
            Self {
                runtime: AgentRuntime::Claude,
                status: RuntimeStatus {
                    runtime: "claude".into(),
                    installed: false,
                    auth_status: None,
                },
                models: vec![],
            }
        }
    }

    impl Driver for FakeV1Driver {
        fn runtime(&self) -> AgentRuntime {
            self.runtime
        }

        fn supports_stdin_notification(&self) -> bool {
            true
        }

        fn mcp_tool_prefix(&self) -> &str {
            "mcp__"
        }

        fn spawn(&self, _ctx: &SpawnContext) -> anyhow::Result<Child> {
            Ok(Command::new("cat")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?)
        }

        fn parse_line(&self, _line: &str) -> Vec<ParsedEvent> {
            vec![]
        }

        fn encode_stdin_message(&self, text: &str, _session_id: &str) -> Option<String> {
            Some(text.to_string())
        }

        fn build_system_prompt(&self, _config: &AgentConfig, _agent_id: &str) -> String {
            String::new()
        }

        fn tool_display_name(&self, name: &str) -> String {
            name.to_string()
        }

        fn summarize_tool_input(&self, _name: &str, _input: &serde_json::Value) -> String {
            String::new()
        }

        fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
            Ok(self.status.clone())
        }

        fn list_models(&self) -> anyhow::Result<Vec<String>> {
            Ok(self.models.clone())
        }
    }

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test".into(),
            description: None,
            system_prompt: None,
            model: "gpt-4".into(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("."),
            bridge_binary: "bridge".into(),
            server_url: "http://localhost".into(),
        }
    }

    #[tokio::test]
    async fn v1_adapter_probe_maps_v1_status_to_v2_probe() {
        // Authed
        let driver = Arc::new(FakeV1Driver::authed(vec![]));
        let adapter = V1DriverAdapter::new(driver);
        let probe = adapter.probe().await.unwrap();
        assert_eq!(probe.auth, ProbeAuth::Authed);
        assert_eq!(probe.transport, TransportKind::AcpAdapter);
        assert_eq!(probe.capabilities, CapabilitySet::MODEL_LIST);

        // Unauthed (installed but no auth)
        let driver = Arc::new(FakeV1Driver::unauthed());
        let adapter = V1DriverAdapter::new(driver);
        let probe = adapter.probe().await.unwrap();
        assert_eq!(probe.auth, ProbeAuth::Unauthed);

        // Not installed
        let driver = Arc::new(FakeV1Driver::not_installed());
        let adapter = V1DriverAdapter::new(driver);
        let probe = adapter.probe().await.unwrap();
        assert_eq!(probe.auth, ProbeAuth::NotInstalled);
    }

    #[tokio::test]
    async fn v1_adapter_attach_returns_idle_handle() {
        let driver = Arc::new(FakeV1Driver::authed(vec![]));
        let adapter = V1DriverAdapter::new(driver);
        let result = adapter.attach("agent-1".into(), test_spec()).await.unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }

    #[tokio::test]
    async fn v1_adapter_list_models_maps_strings_to_model_info() {
        let driver = Arc::new(FakeV1Driver::authed(vec!["gpt-4".into(), "gpt-3.5".into()]));
        let adapter = V1DriverAdapter::new(driver);
        let models = adapter.list_models().await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4");
        assert_eq!(models[0].display_name, "gpt-4");
        assert!(!models[0].supports_reasoning_effort);
        assert_eq!(models[1].id, "gpt-3.5");
    }

    #[tokio::test]
    async fn v1_adapter_cancel_returns_session_replaced_with_empty_id() {
        let driver = Arc::new(FakeV1Driver::authed(vec![]));
        let adapter = V1DriverAdapter::new(driver);
        let result = adapter
            .attach("agent-cancel".into(), test_spec())
            .await
            .unwrap();
        let mut handle = result.handle;
        let outcome = handle.cancel(RunId::new_v4()).await.unwrap();
        match outcome {
            CancelOutcome::SessionReplaced { new_session_id } => {
                assert!(new_session_id.is_empty());
            }
            other => panic!("expected SessionReplaced, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn v1_adapter_close_is_idempotent() {
        let driver = Arc::new(FakeV1Driver::authed(vec![]));
        let adapter = V1DriverAdapter::new(driver);
        let result = adapter
            .attach("agent-close".into(), test_spec())
            .await
            .unwrap();
        let mut handle = result.handle;
        handle.close().await.unwrap();
        // Second close should be a no-op.
        handle.close().await.unwrap();
    }

    #[tokio::test]
    async fn v1_adapter_login_returns_error() {
        let driver = Arc::new(FakeV1Driver::authed(vec![]));
        let adapter = V1DriverAdapter::new(driver);
        let result = adapter.login().await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("does not support login"));
    }
}
