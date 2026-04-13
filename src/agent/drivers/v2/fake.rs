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
        let (events, event_tx) = EventFanOut::new();
        let handle = (self.handle_factory)(key, spec, events.clone(), event_tx);
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
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
    next_session: u32,
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
            next_session: 1,
        }
    }

    pub fn with_prompt_responses(mut self, responses: Vec<Vec<DriverEvent>>) -> Self {
        self.prompt_responses = VecDeque::from(responses);
        self
    }

    fn emit(&self, event: DriverEvent) {
        // Best-effort send; tests subscribe before driving the handle.
        let _ = self.event_tx.try_send(event);
    }

    fn session_id(&self) -> Option<&str> {
        match &self.state {
            AgentState::Active { session_id } => Some(session_id),
            AgentState::PromptInFlight { session_id, .. } => Some(session_id),
            _ => None,
        }
    }
}

#[async_trait]
impl AgentHandle for FakeHandle {
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
        self.state = AgentState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        let session_id = opts
            .resume_session_id
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

    async fn new_session(&mut self) -> anyhow::Result<SessionId> {
        self.next_session += 1;
        let id = format!("fake-session-{}", self.next_session);

        self.emit(DriverEvent::SessionAttached {
            key: self.key.clone(),
            session_id: id.clone(),
        });
        self.state = AgentState::Active {
            session_id: id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Active {
                session_id: id.clone(),
            },
        });

        Ok(id)
    }

    async fn resume_session(&mut self, id: SessionId) -> anyhow::Result<()> {
        self.emit(DriverEvent::SessionAttached {
            key: self.key.clone(),
            session_id: id.clone(),
        });
        self.state = AgentState::Active {
            session_id: id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Active {
                session_id: id.clone(),
            },
        });

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
                run_id,
                item: AgentEventItem::Text {
                    text: "fake response".to_string(),
                },
            });
            self.emit(DriverEvent::Output {
                key: self.key.clone(),
                run_id,
                item: AgentEventItem::TurnEnd,
            });
            self.emit(DriverEvent::Completed {
                key: self.key.clone(),
                run_id,
                result: RunResult {
                    session_id: session_id.clone(),
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
        if let AgentState::PromptInFlight {
            run_id,
            session_id,
        } = &self.state
        {
            let run_id = *run_id;
            let session_id = session_id.clone();

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
            bridge_binary: String::new(),
            server_url: String::new(),
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

        result.handle.start(StartOpts::default(), None).await.unwrap();

        // Starting lifecycle
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(ev, DriverEvent::Lifecycle { state: AgentState::Starting, .. }));

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
        assert!(matches!(ev, DriverEvent::Lifecycle { state: AgentState::Active { .. }, .. }));

        assert!(matches!(result.handle.state(), AgentState::Active { .. }));
    }

    #[tokio::test]
    async fn test_fake_handle_prompt_emits_events() {
        let driver = FakeDriver::new(AgentRuntime::Claude).with_handle_factory(
            |key, _spec, events, event_tx| {
                FakeHandle::new(key, events, event_tx)
                    .with_prompt_responses(vec![vec![DriverEvent::Output {
                        key: "agent-1".to_string(),
                        run_id: RunId::new_v4(),
                        item: AgentEventItem::Text {
                            text: "custom response".to_string(),
                        },
                    }]])
            },
        );

        let mut result = driver
            .attach("agent-1".to_string(), test_spec())
            .await
            .unwrap();
        let mut rx = result.events.subscribe();

        result.handle.start(StartOpts::default(), None).await.unwrap();

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
        assert!(matches!(ev, DriverEvent::Lifecycle { state: AgentState::PromptInFlight { .. }, .. }));

        // Custom output event
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        match &ev {
            DriverEvent::Output { item: AgentEventItem::Text { text }, .. } => {
                assert_eq!(text, "custom response");
            }
            other => panic!("expected Output(Text), got {other:?}"),
        }

        // Active lifecycle (back from prompt)
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(ev, DriverEvent::Lifecycle { state: AgentState::Active { .. }, .. }));
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

        result.handle.start(StartOpts::default(), None).await.unwrap();

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
        assert!(matches!(ev, DriverEvent::Lifecycle { state: AgentState::PromptInFlight { .. }, .. }));

        // Default Text output
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        match &ev {
            DriverEvent::Output { item: AgentEventItem::Text { text }, .. } => {
                assert_eq!(text, "fake response");
            }
            other => panic!("expected Output(Text), got {other:?}"),
        }

        // TurnEnd
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        assert!(matches!(ev, DriverEvent::Output { item: AgentEventItem::TurnEnd, .. }));

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
        assert!(matches!(ev, DriverEvent::Lifecycle { state: AgentState::Active { .. }, .. }));
    }
}
