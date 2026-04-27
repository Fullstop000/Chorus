//! Shared `AcpNativeHandle` — one per ACP session.
//!
//! Multiple handles may share a single [`super::AcpNativeCore`] when the
//! agent has more than one session multiplexed over the same child process.
//! The handle owns its own session id, lifecycle mirror state, and ties
//! itself to the core's per-session entry in `SharedReaderState::sessions`.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::super::acp_protocol;
use super::super::{
    AgentKey, CancelOutcome, DriverEvent, FinishReason, ProcessState, PromptReq, RunId, RunResult,
    Session, SessionId,
};

use super::core::AcpNativeCore;
use super::state::{PendingRequest, SessionState, SharedReaderState};
use super::InitPromptStrategy;

pub(crate) struct AcpNativeHandle {
    core: Arc<AcpNativeCore>,
    /// Session id assigned to this handle. None until `run()` completes.
    /// Populated from the `session/new` or `session/load` response.
    session_id: Option<SessionId>,
    /// Lifecycle mirror for `process_state()` calls that don't want to
    /// take the shared mutex. Kept in sync with
    /// `core.shared.sessions[session_id]`.
    state: ProcessState,
    /// For resume paths, the caller supplies this up-front via
    /// `open_session(SessionIntent::Resume(_))`. The handle's `run()`
    /// sends `session/load` with this id.
    preassigned_session_id: Option<SessionId>,
}

impl AcpNativeHandle {
    pub fn new(core: Arc<AcpNativeCore>, preassigned_session_id: Option<SessionId>) -> Self {
        Self {
            core,
            session_id: None,
            state: ProcessState::Idle,
            preassigned_session_id,
        }
    }

    /// Test-only: simulate a post-`run()` handle state without going through
    /// the full ensure_started/session_new round-trip. Used by the shared
    /// tests in `acp_native::tests` that exercise close/cancel paths.
    #[cfg(test)]
    pub(super) fn set_session_for_test(&mut self, sid: &str, state: ProcessState) {
        self.session_id = Some(sid.to_string());
        self.state = state;
    }

    fn emit(&self, event: DriverEvent) {
        self.core.emit(event);
    }

    async fn alloc_id(&self) -> u64 {
        let mut inner = self.core.inner.lock().await;
        let id = inner.next_request_id;
        inner.next_request_id += 1;
        id
    }

    async fn acquire_stdin_and_shared(
        &self,
    ) -> anyhow::Result<(mpsc::Sender<String>, Arc<Mutex<SharedReaderState>>)> {
        let inner = self.core.inner.lock().await;
        let stdin_tx = inner.stdin_tx.clone().ok_or_else(|| {
            anyhow!(
                "{}: stdin not available — ensure_started() must complete first",
                self.core.cfg.name
            )
        })?;
        let shared = inner.shared.clone().ok_or_else(|| {
            anyhow!("{}: shared reader state missing", self.core.cfg.name)
        })?;
        Ok((stdin_tx, shared))
    }

    async fn send_session_new(&self) -> anyhow::Result<String> {
        let (stdin_tx, shared) = self.acquire_stdin_and_shared().await?;
        let id = self.alloc_id().await;
        let (tx, rx) = oneshot::channel();
        let mcp_servers = (self.core.cfg.build_session_new_mcp_servers)(
            &self.core.spec.bridge_endpoint,
            &self.core.key,
        );
        let params = serde_json::json!({
            "cwd": self.core.spec.working_directory,
            "mcpServers": mcp_servers,
        });
        {
            let mut s = shared.lock().unwrap();
            s.pending
                .insert(id, PendingRequest::SessionNew { responder: tx });
        }
        let req = acp_protocol::build_session_new_request(id, params);
        stdin_tx
            .send(req)
            .await
            .with_context(|| format!("{}: stdin channel closed", self.core.cfg.name))?;
        let driver = self.core.cfg.name;
        rx.await
            .map_err(|_| anyhow!("{driver}: reader task dropped before session/new response"))?
            .map_err(|msg| anyhow!("{driver}: session/new failed: {msg}"))
    }

    async fn send_session_load(&self, sid: &str) -> anyhow::Result<String> {
        let (stdin_tx, shared) = self.acquire_stdin_and_shared().await?;
        let id = self.alloc_id().await;
        let (tx, rx) = oneshot::channel();
        // session/load mcpServers shape diverges per runtime: gemini sends
        // an empty array, kimi resends the full set. ACP spec leaves this
        // implementation-defined; preserve each runtime's behavior.
        let mcp_servers: Value = if self.core.cfg.session_load_includes_mcp {
            (self.core.cfg.build_session_new_mcp_servers)(
                &self.core.spec.bridge_endpoint,
                &self.core.key,
            )
        } else {
            serde_json::json!([])
        };
        let params = serde_json::json!({
            "cwd": self.core.spec.working_directory,
            "mcpServers": mcp_servers,
        });
        {
            let mut s = shared.lock().unwrap();
            s.pending.insert(
                id,
                PendingRequest::SessionLoad {
                    expected_session_id: sid.to_string(),
                    responder: tx,
                },
            );
        }
        let req = acp_protocol::build_session_load_request(id, sid, params);
        stdin_tx
            .send(req)
            .await
            .with_context(|| format!("{}: stdin channel closed", self.core.cfg.name))?;
        let driver = self.core.cfg.name;
        rx.await
            .map_err(|_| anyhow!("{driver}: reader task dropped before session/load response"))?
            .map_err(|msg| anyhow!("{driver}: session/load failed: {msg}"))
    }

    async fn register_session_in_shared_state(&self, session_id: &str) {
        let inner = self.core.inner.lock().await;
        if let Some(ref shared) = inner.shared {
            let mut s = shared.lock().unwrap();
            s.sessions
                .entry(session_id.to_string())
                .or_insert_with(|| SessionState::new(session_id));
        }
    }

    async fn run_inner(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        let cfg = self.core.cfg;

        if cfg.emit_starting_lifecycle {
            self.state = ProcessState::Starting;
            self.emit(DriverEvent::Lifecycle {
                key: self.core.key.clone(),
                state: ProcessState::Starting,
            });
        }

        // Lazy, race-safe bootstrap. The first handle to call run_inner
        // spawns the child and sends `initialize`; concurrent handles
        // wait for the race-winner and then proceed to session minting.
        self.core.ensure_started().await?;

        let session_id = if let Some(ref preassigned) = self.preassigned_session_id.clone() {
            self.send_session_load(preassigned).await?
        } else {
            self.send_session_new().await?
        };

        self.register_session_in_shared_state(&session_id).await;
        self.session_id = Some(session_id.clone());
        self.state = ProcessState::Active {
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::SessionAttached {
            key: self.core.key.clone(),
            session_id: session_id.clone(),
        });
        self.emit(DriverEvent::Lifecycle {
            key: self.core.key.clone(),
            state: self.state.clone(),
        });

        match cfg.init_prompt_strategy {
            InitPromptStrategy::Immediate => {
                // If the driver carries a standing prompt, ALWAYS send a
                // prompt — either standing-only (when init_prompt is
                // None) or `standing + "---" + init_prompt` (when
                // present). Without a standing prompt, only fire when the
                // caller passed an explicit init_prompt.
                let first_turn = match (cfg.build_first_prompt_prefix, init_prompt) {
                    (Some(build), Some(req)) => Some(PromptReq {
                        text: format!("{}\n\n---\n\n{}", build(&self.core.spec), req.text),
                        attachments: req.attachments,
                    }),
                    (Some(build), None) => Some(PromptReq {
                        text: build(&self.core.spec),
                        attachments: Vec::new(),
                    }),
                    (None, Some(req)) => Some(req),
                    (None, None) => None,
                };
                if let Some(req) = first_turn {
                    self.prompt(req).await?;
                }
            }
            InitPromptStrategy::Deferred => {
                // Wait for the caller to invoke `prompt()` explicitly.
                // `init_prompt` is intentionally ignored in this
                // strategy — opencode (PR2) is the consumer.
            }
        }

        Ok(())
    }
}

impl Drop for AcpNativeHandle {
    fn drop(&mut self) {
        // `Session::close()` is the authoritative lifecycle shutdown
        // path. A dropped handle may follow an explicit close(); emitting
        // here would duplicate the terminal Closed event. The core's
        // `Drop` handles child termination when the last `Arc` is
        // released.
    }
}

#[async_trait]
impl Session for AcpNativeHandle {
    fn key(&self) -> &AgentKey {
        &self.core.key
    }

    fn session_id(&self) -> Option<&str> {
        match &self.state {
            ProcessState::Active { session_id } => Some(session_id.as_str()),
            ProcessState::PromptInFlight { session_id, .. } => Some(session_id.as_str()),
            _ => self
                .session_id
                .as_deref()
                .or(self.preassigned_session_id.as_deref()),
        }
    }

    fn process_state(&self) -> ProcessState {
        if let Some(ref sid) = self.session_id {
            if let Ok(inner) = self.core.inner.try_lock() {
                if let Some(shared) = inner.shared.as_ref() {
                    let shared = shared.lock().unwrap();
                    if let Some(session) = shared.sessions.get(sid) {
                        return session.state.clone();
                    }
                }
            }
        }
        self.state.clone()
    }

    async fn run(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        self.run_inner(init_prompt).await
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let driver = self.core.cfg.name;
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("{driver}: prompt() called before run()"))?;

        let run_id = RunId::new_v4();
        let request_id = self.alloc_id().await;

        let (stdin_tx, shared) = {
            let inner = self.core.inner.lock().await;
            let tx = inner.stdin_tx.clone().ok_or_else(|| {
                anyhow!("{driver}: stdin not available — handle not started")
            })?;
            let shared = inner
                .shared
                .clone()
                .ok_or_else(|| anyhow!("{driver}: shared state missing"))?;
            (tx, shared)
        };

        {
            let mut s = shared.lock().unwrap();
            s.pending.insert(
                request_id,
                PendingRequest::Prompt {
                    session_id: session_id.clone(),
                    run_id,
                },
            );
            let slot = s
                .sessions
                .entry(session_id.clone())
                .or_insert_with(|| SessionState::new(&session_id));
            slot.run_id = Some(run_id);
            slot.state = ProcessState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            };
        }

        self.state = ProcessState::PromptInFlight {
            run_id,
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.core.key.clone(),
            state: ProcessState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            },
        });

        let prompt_req =
            acp_protocol::build_session_prompt_request(request_id, &session_id, &req.text);
        stdin_tx
            .send(prompt_req)
            .await
            .with_context(|| format!("{driver}: stdin channel closed"))?;

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        // Authoritative session state lives in shared.sessions keyed by
        // this handle's session id — self.state may lag the reader.
        let Some(sid) = self.session_id.clone() else {
            return Ok(CancelOutcome::NotInFlight);
        };
        let shared = {
            let inner = self.core.inner.lock().await;
            inner.shared.clone()
        };
        let Some(shared) = shared else {
            return Ok(CancelOutcome::NotInFlight);
        };

        let (run_id, session_id) = {
            let mut s = shared.lock().unwrap();
            let slot = match s.sessions.get_mut(&sid) {
                Some(slot) => slot,
                None => return Ok(CancelOutcome::NotInFlight),
            };
            match &slot.state {
                ProcessState::PromptInFlight { run_id, session_id } => {
                    let rid = *run_id;
                    let psid = session_id.clone();
                    slot.run_id = None;
                    slot.state = ProcessState::Active {
                        session_id: psid.clone(),
                    };
                    (rid, psid)
                }
                _ => return Ok(CancelOutcome::NotInFlight),
            }
        };

        self.emit(DriverEvent::Completed {
            key: self.core.key.clone(),
            session_id: session_id.clone(),
            run_id,
            result: RunResult {
                finish_reason: FinishReason::Cancelled,
            },
        });

        self.state = ProcessState::Active { session_id };
        Ok(CancelOutcome::Aborted)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.state, ProcessState::Closed) {
            return Ok(());
        }

        // Drop this handle's session slot from shared state so
        // `pick_session` / `pick_session_and_run` stop routing events to
        // a dead handle.
        //
        // Under the same lock, compute `all_sessions_closed` — true iff
        // every remaining session entry is Closed (or the map is empty)
        // AND no session/new or session/load is currently pending.
        // Teardown of the shared child + fan-out + registry entry is
        // gated on this: a close with a sibling session still mid-prompt
        // must NOT kill the child.
        let (all_sessions_closed, shared_opt) = {
            let shared_opt = {
                let inner = self.core.inner.lock().await;
                inner.shared.clone()
            };
            if let Some(ref shared) = shared_opt {
                let mut s = shared.lock().unwrap();
                if let Some(ref sid) = self.session_id {
                    s.sessions.remove(sid);
                }
                let all_closed = s
                    .sessions
                    .values()
                    .all(|slot| matches!(slot.state, ProcessState::Closed));
                let no_pending_session_creation = !s.pending.values().any(|p| {
                    matches!(
                        p,
                        PendingRequest::SessionNew { .. } | PendingRequest::SessionLoad { .. }
                    )
                });
                (
                    all_closed && no_pending_session_creation,
                    Some(shared.clone()),
                )
            } else {
                // start() never completed; nothing to route to. Treat as
                // "all closed" so the teardown path still fires (the
                // core has no live sessions to preserve).
                (true, None)
            }
        };

        self.state = ProcessState::Closed;

        // Always emit a per-session Closed lifecycle event so subscribers
        // see this handle retire — independent of whether the shared
        // child teardown below fires.
        self.emit(DriverEvent::Lifecycle {
            key: self.core.key.clone(),
            state: ProcessState::Closed,
        });

        if all_sessions_closed {
            if let Some(ref shared) = shared_opt {
                let s = shared.lock().unwrap();
                // Flip BEFORE SIGTERM so a reader racing our abort()
                // toward the EOF `Lifecycle::Closed` emission sees the
                // flag and skips (no double-emit).
                s.closed_emitted.store(true, Ordering::SeqCst);
            }

            let key = self.core.key.clone();
            {
                let mut inner = self.core.inner.lock().await;
                if let Some(ref mut child) = inner.owned.child {
                    let pid = child.id();
                    let _ = nix::sys::signal::kill(
                        nix::unistd::Pid::from_raw(pid as i32),
                        nix::sys::signal::Signal::SIGTERM,
                    );
                }
                inner.owned.child = None;
                inner.stdin_tx = None;
                for handle in inner.owned.reader_handles.drain(..) {
                    handle.abort();
                }
            }
            self.core.events.close();
            (self.core.cfg.registry)().remove(&key);
        }

        Ok(())
    }
}
