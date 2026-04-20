//! In-memory test doubles for v2 driver traits.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::bail;
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::agent::AgentRuntime;

use super::*;

// ---------------------------------------------------------------------------
// FakeDriver
// ---------------------------------------------------------------------------

pub struct FakeDriver {
    runtime: AgentRuntime,
    probe_result: RuntimeProbe,
    models: Vec<ModelInfo>,
    handle_factory: Arc<
        dyn Fn(AgentKey, AgentSpec, EventStreamHandle, mpsc::Sender<DriverEvent>) -> FakeHandle
            + Send
            + Sync,
    >,
    /// Per-agent shared process state, keyed by agent key. Each entry represents
    /// one simulated driver process that can host multiple concurrent sessions.
    /// Sessions on the same agent share the event stream, so consumers see all
    /// events from one channel and route on the `session_id` carried directly
    /// on every run-scoped event (`Output`, `Completed`, `Failed`) — no
    /// external `run_id → session_id` map is needed.
    agent_instances:
        Arc<std::sync::Mutex<std::collections::HashMap<AgentKey, Arc<FakeAgentProcess>>>>,
}

/// Simulated runtime process shared by all sessions on one agent. Holds the
/// event channel both attach and new_session sessions write into, plus a
/// next-session counter so session ids are monotonically unique per agent.
pub struct FakeAgentProcess {
    key: AgentKey,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    next_session: std::sync::Mutex<u32>,
}

impl FakeAgentProcess {
    fn mint_session_id(&self) -> SessionId {
        // Counter starts at 1 and increments before format, so the first
        // mint produces `fake-session-2`. This reserves `fake-session-1`
        // for the hardcoded attach-path default in `FakeHandle::start()`,
        // guaranteeing mint ids can't collide with it.
        let mut n = self.next_session.lock().unwrap();
        *n += 1;
        format!("fake-session-{}", *n)
    }
}

impl FakeDriver {
    pub fn new(runtime: AgentRuntime) -> Self {
        Self {
            runtime,
            probe_result: RuntimeProbe {
                auth: ProbeAuth::Authed,
                transport: TransportKind::AcpNative,
                capabilities: CapabilitySet::MODEL_LIST,
            },
            models: vec![],
            handle_factory: Arc::new(|key, _spec, events, event_tx| {
                FakeHandle::new(key, events, event_tx)
            }),
            agent_instances: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn with_probe(mut self, probe: RuntimeProbe) -> Self {
        self.probe_result = probe;
        self
    }

    pub fn with_models(mut self, models: Vec<ModelInfo>) -> Self {
        self.models = models;
        self
    }

    pub fn with_handle_factory<F>(mut self, f: F) -> Self
    where
        F: Fn(AgentKey, AgentSpec, EventStreamHandle, mpsc::Sender<DriverEvent>) -> FakeHandle
            + Send
            + Sync
            + 'static,
    {
        self.handle_factory = Arc::new(f);
        self
    }

    /// Look up or create the simulated process for this agent. Returns the
    /// shared process Arc; sessions share the process's event channel.
    fn ensure_process(&self, key: &AgentKey) -> Arc<FakeAgentProcess> {
        let mut guard = self.agent_instances.lock().unwrap();
        if let Some(existing) = guard.get(key) {
            return Arc::clone(existing);
        }
        let (events, event_tx) = EventFanOut::new();
        let proc = Arc::new(FakeAgentProcess {
            key: key.clone(),
            events,
            event_tx,
            // Start at 1 so `mint_session_id` first produces "fake-session-2"
            // and the attach-path default literal "fake-session-1" can never
            // collide with a minted id.
            next_session: std::sync::Mutex::new(1),
        });
        guard.insert(key.clone(), Arc::clone(&proc));
        proc
    }
}

#[async_trait]
impl RuntimeDriver for FakeDriver {
    fn runtime(&self) -> AgentRuntime {
        self.runtime
    }

    async fn probe(&self) -> anyhow::Result<RuntimeProbe> {
        Ok(self.probe_result.clone())
    }

    async fn login(&self) -> anyhow::Result<LoginOutcome> {
        Ok(LoginOutcome::Completed)
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>> {
        Ok(vec![])
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(self.models.clone())
    }

    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>> {
        Ok(vec![])
    }

    async fn attach(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let proc = self.ensure_process(&key);
        let handle = (self.handle_factory)(key, spec, proc.events.clone(), proc.event_tx.clone());
        Ok(AttachResult {
            handle: Box::new(handle),
            events: proc.events.clone(),
        })
    }

    async fn new_session(&self, key: AgentKey, spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let proc = self.ensure_process(&key);
        let session_id = proc.mint_session_id();
        let mut handle =
            (self.handle_factory)(key, spec, proc.events.clone(), proc.event_tx.clone());
        // Pre-bind the session id so the handle behaves as if start() already
        // resumed onto this specific id. A future step/test may flip to Active
        // explicitly via start; for now, surface the id through session_id().
        handle.preassign_session(session_id);
        Ok(AttachResult {
            handle: Box::new(handle),
            events: proc.events.clone(),
        })
    }

    async fn resume_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        session_id: SessionId,
    ) -> anyhow::Result<AttachResult> {
        let proc = self.ensure_process(&key);
        let mut handle =
            (self.handle_factory)(key, spec, proc.events.clone(), proc.event_tx.clone());
        handle.preassign_session(session_id);
        Ok(AttachResult {
            handle: Box::new(handle),
            events: proc.events.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// FakeHandle
// ---------------------------------------------------------------------------

pub struct FakeHandle {
    key: AgentKey,
    state: AgentState,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    prompt_responses: VecDeque<Vec<DriverEvent>>,
    /// If Some, `start` will attach this session id instead of minting the
    /// default `fake-session-1`. Set by `FakeDriver::new_session` /
    /// `resume_session` to hand the handle a pre-assigned id so the caller
    /// can run multiple sessions concurrently on the same agent.
    preassigned_session_id: Option<SessionId>,
}

impl FakeHandle {
    pub fn new(
        key: AgentKey,
        events: EventStreamHandle,
        event_tx: mpsc::Sender<DriverEvent>,
    ) -> Self {
        Self {
            key,
            state: AgentState::Idle,
            events,
            event_tx,
            prompt_responses: VecDeque::new(),
            preassigned_session_id: None,
        }
    }

    pub fn with_prompt_responses(mut self, responses: Vec<Vec<DriverEvent>>) -> Self {
        self.prompt_responses = VecDeque::from(responses);
        self
    }

    /// Pre-bind a session id so `start` attaches to it rather than minting
    /// the default. Used by `FakeDriver::new_session` / `resume_session` to
    /// give each spawned handle its own session id while sharing the agent's
    /// process event stream.
    pub(super) fn preassign_session(&mut self, id: SessionId) {
        self.preassigned_session_id = Some(id);
    }

    fn emit(&self, event: DriverEvent) {
        // Best-effort send; tests subscribe before driving the handle.
        let _ = self.event_tx.try_send(event);
    }
}

#[async_trait]
impl AgentSessionHandle for FakeHandle {
    fn key(&self) -> &AgentKey {
        &self.key
    }

    fn session_id(&self) -> Option<&str> {
        match &self.state {
            AgentState::Active { session_id } => Some(session_id),
            AgentState::PromptInFlight { session_id, .. } => Some(session_id),
            _ => self.preassigned_session_id.as_deref(),
        }
    }

    fn state(&self) -> AgentState {
        self.state.clone()
    }

    async fn start(
        &mut self,
        opts: StartOpts,
        init_prompt: Option<PromptReq>,
    ) -> anyhow::Result<()> {
        self.state = AgentState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        // Precedence: explicit `opts.resume_session_id` > preassigned (from
        // FakeDriver::new_session / resume_session) > default.
        let session_id = opts
            .resume_session_id
            .or_else(|| self.preassigned_session_id.clone())
            .unwrap_or_else(|| "fake-session-1".to_string());

        self.emit(DriverEvent::SessionAttached {
            key: self.key.clone(),
            session_id: session_id.clone(),
        });

        self.state = AgentState::Active {
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Active {
                session_id: session_id.clone(),
            },
        });

        if let Some(req) = init_prompt {
            self.prompt(req).await?;
        }

        Ok(())
    }

    async fn prompt(&mut self, _req: PromptReq) -> anyhow::Result<RunId> {
        let session_id = match &self.state {
            AgentState::Active { session_id } => session_id.clone(),
            _ => bail!("not active"),
        };

        let run_id = RunId::new_v4();

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

        if let Some(events) = self.prompt_responses.pop_front() {
            for event in events {
                self.emit(event);
            }
        } else {
            self.emit(DriverEvent::Output {
                key: self.key.clone(),
                session_id: session_id.clone(),
                run_id,
                item: AgentEventItem::Text {
                    text: "fake response".to_string(),
                },
            });
            self.emit(DriverEvent::Output {
                key: self.key.clone(),
                session_id: session_id.clone(),
                run_id,
                item: AgentEventItem::TurnEnd,
            });
            self.emit(DriverEvent::Completed {
                key: self.key.clone(),
                session_id: session_id.clone(),
                run_id,
                result: RunResult {
                    finish_reason: FinishReason::Natural,
                },
            });
        }

        self.state = AgentState::Active {
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Active {
                session_id: session_id.clone(),
            },
        });

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        if let AgentState::PromptInFlight { run_id, session_id } = &self.state {
            let run_id = *run_id;
            let session_id = session_id.clone();

            self.emit(DriverEvent::Completed {
                key: self.key.clone(),
                session_id: session_id.clone(),
                run_id,
                result: RunResult {
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

        self.state = AgentState::Closed;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Closed,
        });
        self.events.close();

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
    use std::time::Duration;
    use tokio::time::timeout;

    fn test_spec() -> AgentSpec {
        AgentSpec {
            display_name: "test-agent".to_string(),
            description: None,
            system_prompt: None,
            model: "fake-model".to_string(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_endpoint: "http://127.0.0.1:1".to_string(),
        }
    }

    #[tokio::test]
    async fn test_fake_driver_probe() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let probe = driver.probe().await.unwrap();
        assert_eq!(probe.auth, ProbeAuth::Authed);
        assert_eq!(probe.transport, TransportKind::AcpNative);
        assert!(probe.capabilities.contains(CapabilitySet::MODEL_LIST));
    }

    #[tokio::test]
    async fn test_fake_driver_attach_returns_idle_handle() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let result = driver
            .attach("agent-1".to_string(), test_spec())
            .await
            .unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
    }

    #[tokio::test]
    async fn test_fake_handle_start_lifecycle() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let mut result = driver
            .attach("agent-1".to_string(), test_spec())
            .await
            .unwrap();
        let mut rx = result.events.subscribe();

        result
            .handle
            .start(StartOpts::default(), None)
            .await
            .unwrap();

        // Starting lifecycle
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(
            ev,
            DriverEvent::Lifecycle {
                state: AgentState::Starting,
                ..
            }
        ));

        // SessionAttached
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(ev, DriverEvent::SessionAttached { .. }));

        // Active lifecycle
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(
            ev,
            DriverEvent::Lifecycle {
                state: AgentState::Active { .. },
                ..
            }
        ));

        assert!(matches!(result.handle.state(), AgentState::Active { .. }));
    }

    #[tokio::test]
    async fn test_fake_handle_prompt_emits_events() {
        let driver = FakeDriver::new(AgentRuntime::Claude).with_handle_factory(
            |key, _spec, events, event_tx| {
                FakeHandle::new(key, events, event_tx).with_prompt_responses(vec![vec![
                    DriverEvent::Output {
                        key: "agent-1".to_string(),
                        session_id: "fake-session-1".to_string(),
                        run_id: RunId::new_v4(),
                        item: AgentEventItem::Text {
                            text: "custom response".to_string(),
                        },
                    },
                ]])
            },
        );

        let mut result = driver
            .attach("agent-1".to_string(), test_spec())
            .await
            .unwrap();
        let mut rx = result.events.subscribe();

        result
            .handle
            .start(StartOpts::default(), None)
            .await
            .unwrap();

        // Drain the start lifecycle events (Starting, SessionAttached, Active)
        for _ in 0..3 {
            timeout(Duration::from_millis(500), rx.recv())
                .await
                .expect("timeout")
                .expect("closed");
        }

        let req = PromptReq {
            text: "hello".to_string(),
            attachments: vec![],
        };
        result.handle.prompt(req).await.unwrap();

        // PromptInFlight lifecycle
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(
            ev,
            DriverEvent::Lifecycle {
                state: AgentState::PromptInFlight { .. },
                ..
            }
        ));

        // Custom output event
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        match &ev {
            DriverEvent::Output {
                item: AgentEventItem::Text { text },
                ..
            } => {
                assert_eq!(text, "custom response");
            }
            other => panic!("expected Output(Text), got {other:?}"),
        }

        // Active lifecycle (back from prompt)
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(
            ev,
            DriverEvent::Lifecycle {
                state: AgentState::Active { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn test_fake_handle_close_idempotent() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let mut result = driver
            .attach("agent-1".to_string(), test_spec())
            .await
            .unwrap();

        result.handle.close().await.unwrap();
        assert!(matches!(result.handle.state(), AgentState::Closed));

        // Second close is a no-op
        result.handle.close().await.unwrap();
        assert!(matches!(result.handle.state(), AgentState::Closed));
    }

    #[tokio::test]
    async fn test_fake_handle_prompt_default_response() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let mut result = driver
            .attach("agent-1".to_string(), test_spec())
            .await
            .unwrap();
        let mut rx = result.events.subscribe();

        result
            .handle
            .start(StartOpts::default(), None)
            .await
            .unwrap();

        // Drain start events
        for _ in 0..3 {
            timeout(Duration::from_millis(500), rx.recv())
                .await
                .expect("timeout")
                .expect("closed");
        }

        let req = PromptReq {
            text: "hello".to_string(),
            attachments: vec![],
        };
        result.handle.prompt(req).await.unwrap();

        // PromptInFlight
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(
            ev,
            DriverEvent::Lifecycle {
                state: AgentState::PromptInFlight { .. },
                ..
            }
        ));

        // Default Text output
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        match &ev {
            DriverEvent::Output {
                item: AgentEventItem::Text { text },
                ..
            } => {
                assert_eq!(text, "fake response");
            }
            other => panic!("expected Output(Text), got {other:?}"),
        }

        // TurnEnd
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(
            ev,
            DriverEvent::Output {
                item: AgentEventItem::TurnEnd,
                ..
            }
        ));

        // Completed(Natural)
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        match &ev {
            DriverEvent::Completed { result, .. } => {
                assert_eq!(result.finish_reason, FinishReason::Natural);
            }
            other => panic!("expected Completed, got {other:?}"),
        }

        // Active lifecycle
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(
            ev,
            DriverEvent::Lifecycle {
                state: AgentState::Active { .. },
                ..
            }
        ));
    }

    // -----------------------------------------------------------------------
    // Multi-session spike (Phase 0.9 Stage 1)
    //
    // These tests prove the 2-tier architecture: RuntimeDriver mints sessions
    // via new_session() / resume_session(), each returning its own
    // AgentSessionHandle. Multiple handles for the same agent share one
    // simulated process and its event stream; each session is independent in
    // state and can be prompted without cross-contamination.
    // -----------------------------------------------------------------------

    /// Session ids minted by `new_session` must be distinct and sequential per
    /// agent, so callers can associate a handle with a specific session.
    #[tokio::test]
    async fn multi_session_new_session_returns_distinct_session_ids() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let key = "agent-1".to_string();

        let attach_result = driver.attach(key.clone(), test_spec()).await.unwrap();
        // attach() itself does not mint a session id until start() runs. What
        // we care about here is what new_session() hands back.

        let s1 = driver.new_session(key.clone(), test_spec()).await.unwrap();
        let s2 = driver.new_session(key.clone(), test_spec()).await.unwrap();

        let id1 = s1.handle.session_id().expect("preassigned");
        let id2 = s2.handle.session_id().expect("preassigned");
        assert_ne!(id1, id2, "two new_session calls must yield distinct ids");
        assert!(id1.starts_with("fake-session-"));
        assert!(id2.starts_with("fake-session-"));

        // attach doesn't claim an id pool slot yet (preassigned is None).
        assert_eq!(attach_result.handle.session_id(), None);
    }

    /// Events from all sessions on one agent flow through the same event
    /// stream — proving the "one driver process, many sessions" invariant.
    #[tokio::test]
    async fn multi_session_all_sessions_share_agent_event_stream() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let key = "agent-1".to_string();

        // First attach mints the process + its event channel.
        let attach = driver.attach(key.clone(), test_spec()).await.unwrap();
        let mut rx = attach.events.subscribe();

        // Second session on same agent should write into the SAME channel.
        let mut s2 = driver.new_session(key.clone(), test_spec()).await.unwrap();

        // Starting s2 should emit events that arrive on the original rx.
        s2.handle.start(StartOpts::default(), None).await.unwrap();

        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("s2.start events must reach the shared stream")
            .expect("stream closed");
        assert!(matches!(
            ev,
            DriverEvent::Lifecycle {
                state: AgentState::Starting,
                ..
            }
        ));
    }

    /// Prompting session A must not affect session B's state. This is the
    /// core isolation invariant: claiming task-42 and task-50 on the same
    /// agent should give the LLM two independent contexts.
    #[tokio::test]
    async fn multi_session_sessions_have_isolated_state() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let key = "agent-1".to_string();

        let mut s1 = driver.new_session(key.clone(), test_spec()).await.unwrap();
        let mut s2 = driver.new_session(key.clone(), test_spec()).await.unwrap();

        // Start both.
        s1.handle.start(StartOpts::default(), None).await.unwrap();
        s2.handle.start(StartOpts::default(), None).await.unwrap();

        let id1 = s1.handle.session_id().unwrap().to_string();
        let id2 = s2.handle.session_id().unwrap().to_string();
        assert_ne!(id1, id2);

        // Prompt s1. s2's state should remain Active (not PromptInFlight).
        s1.handle
            .prompt(PromptReq {
                text: "work on task 42".to_string(),
                attachments: vec![],
            })
            .await
            .unwrap();

        // After prompt returns, s1 went PromptInFlight → back to Active.
        // s2 was never touched.
        match s2.handle.state() {
            AgentState::Active { session_id } => {
                assert_eq!(session_id, id2, "s2 must still hold its own session id");
            }
            other => panic!("s2 must remain Active after s1.prompt; got {other:?}"),
        }
    }

    /// `resume_session` attaches a handle to a caller-supplied session id.
    /// Useful when restoring across a restart.
    #[tokio::test]
    async fn multi_session_resume_session_preserves_supplied_id() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let key = "agent-1".to_string();

        let resumed = driver
            .resume_session(key, test_spec(), "stored-session-xyz".to_string())
            .await
            .unwrap();

        assert_eq!(resumed.handle.session_id(), Some("stored-session-xyz"));
    }

    /// End-to-end: three concurrent sessions under one agent, each gets its
    /// own `session_id` on the Completed event, and the `run_id`s never
    /// collide. This is the invariant that proves multiplexing actually works
    /// for the Controlled Session list design.
    #[tokio::test]
    async fn multi_session_three_concurrent_prompts_with_distinct_completed_events() {
        let driver = FakeDriver::new(AgentRuntime::Claude);
        let key = "agent-1".to_string();

        // Subscribe BEFORE spawning sessions so we see their lifecycle events.
        let attach = driver.attach(key.clone(), test_spec()).await.unwrap();
        let mut rx = attach.events.subscribe();

        let mut s1 = driver.new_session(key.clone(), test_spec()).await.unwrap();
        let mut s2 = driver.new_session(key.clone(), test_spec()).await.unwrap();
        let mut s3 = driver.new_session(key.clone(), test_spec()).await.unwrap();

        s1.handle.start(StartOpts::default(), None).await.unwrap();
        s2.handle.start(StartOpts::default(), None).await.unwrap();
        s3.handle.start(StartOpts::default(), None).await.unwrap();

        let id1 = s1.handle.session_id().unwrap().to_string();
        let id2 = s2.handle.session_id().unwrap().to_string();
        let id3 = s3.handle.session_id().unwrap().to_string();

        // Collect these so the test can assert at the end.
        let expected_ids = std::collections::HashSet::from([id1.clone(), id2.clone(), id3.clone()]);
        assert_eq!(expected_ids.len(), 3, "three distinct session ids");

        // Prompt each in sequence. Within each prompt the default fake
        // response emits TurnEnd + Completed carrying the session id.
        s1.handle
            .prompt(PromptReq {
                text: "s1 prompt".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        s2.handle
            .prompt(PromptReq {
                text: "s2 prompt".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        s3.handle
            .prompt(PromptReq {
                text: "s3 prompt".into(),
                attachments: vec![],
            })
            .await
            .unwrap();

        // Drain events until we see 3 Completed events. Assert each carries
        // the correct session_id and the set matches what we minted.
        let mut seen_completed_session_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let drain_deadline = Duration::from_secs(2);
        while seen_completed_session_ids.len() < 3 {
            let ev = timeout(drain_deadline, rx.recv())
                .await
                .expect("timed out waiting for 3 Completed events")
                .expect("stream closed");
            if let DriverEvent::Completed { session_id, .. } = ev {
                assert!(
                    expected_ids.contains(&session_id),
                    "Completed carried unexpected session_id {:?}",
                    session_id
                );
                seen_completed_session_ids.insert(session_id);
            }
        }

        assert_eq!(
            seen_completed_session_ids, expected_ids,
            "every minted session must emit a Completed event carrying its own id"
        );
    }
}
