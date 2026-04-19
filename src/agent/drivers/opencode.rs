//! Native v2 driver for the OpenCode runtime using ACP protocol.
//!
//! # Multi-session architecture (Phase 0.9 Stage 2)
//!
//! A single `opencode acp` child process multiplexes several ACP sessions.
//! We model this with a shared `OpencodeAgentProcess` per agent key:
//!
//! - The first `attach` creates the process shell; `start` spawns the child
//!   and drives the initial `initialize` + `session/new` handshake (ids 1, 2).
//! - `new_session` and `resume_session` reuse the existing child, sending a
//!   fresh `session/new` / `session/load` on the same stdin. The response is
//!   delivered back via a oneshot channel keyed by the JSON-RPC id.
//! - Every handle returned from this driver shares the process's event stream.
//!   Events carry `session_id`, so consumers can route to the owning session.
//!
//! # ID-based response routing (important)
//!
//! `acp_protocol::parse_line` classifies JSON-RPC responses by id: 1 →
//! initialize, 2 → session, ≥3 → prompt. That rule breaks once we send a
//! second `session/new` with id ≥3 — the parser would call it a prompt
//! response. This driver works around the limitation *locally*, without
//! touching the shared protocol parser. We keep a per-process
//! `pending_requests: HashMap<u64, PendingKind>` that records what each
//! outgoing id was. The reader consults it before handing the frame off to
//! the right handler. Notifications (`session/update`,
//! `session/request_permission`) and errors are unaffected — `parse_line`
//! still does the structural work; we only override response classification.

use anyhow::{bail, Context};
use async_trait::async_trait;
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, trace, warn};

use crate::agent::AgentRuntime;
use crate::utils::cmd::{command_exists, run_command};

use super::acp_protocol::{self, AcpParsed, AcpUpdateItem, ToolCallAccumulator};
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
    serde_json::json!({
        "type": "remote",
        "url": crate::bridge::token_mcp_url(bridge_endpoint, token),
    })
}

// ---------------------------------------------------------------------------
// OpencodeDriver
// ---------------------------------------------------------------------------

/// Unit-like driver; the shared per-agent process registry lives in a
/// process-global singleton (see `agent_instances()`). This keeps the
/// constructor call-site compatible with `Arc::new(OpencodeDriver)` in the
/// agent manager, while still letting `new_session` / `resume_session` reach
/// the same `OpencodeAgentProcess` the `attach` on that key created.
pub struct OpencodeDriver;

/// Process-global registry: agent key -> shared runtime process. Populated
/// by `attach`; reused by subsequent `new_session` / `resume_session` calls
/// on the same key. Returning an `Arc` keeps the inner `Mutex` held only
/// briefly.
fn agent_instances() -> &'static Mutex<HashMap<AgentKey, Arc<OpencodeAgentProcess>>> {
    static INSTANCES: std::sync::OnceLock<Mutex<HashMap<AgentKey, Arc<OpencodeAgentProcess>>>> =
        std::sync::OnceLock::new();
    INSTANCES.get_or_init(|| Mutex::new(HashMap::new()))
}

impl OpencodeDriver {
    /// Return the existing shared process for `key`, or create one if it's
    /// the first `attach` for this agent.
    fn ensure_process(&self, key: &AgentKey) -> Arc<OpencodeAgentProcess> {
        let mut guard = agent_instances().lock().unwrap();
        if let Some(existing) = guard.get(key) {
            return Arc::clone(existing);
        }
        let (events, event_tx) = EventFanOut::new();
        let proc = Arc::new(OpencodeAgentProcess {
            key: key.clone(),
            events,
            event_tx,
            child: Mutex::new(None),
            stdin_tx: Mutex::new(None),
            shared: Arc::new(Mutex::new(SharedReaderState::new())),
            next_request_id: AtomicU64::new(3),
            reader_handles: Mutex::new(Vec::new()),
            started: std::sync::atomic::AtomicBool::new(false),
        });
        guard.insert(key.clone(), Arc::clone(&proc));
        proc
    }
}

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
        let proc = self.ensure_process(&key);
        let handle = OpencodeHandle {
            key,
            local_state: AgentState::Idle,
            spec,
            proc: Arc::clone(&proc),
            preassigned_session_id: None,
            bootstraps_process: true,
        };
        Ok(AttachResult {
            handle: Box::new(handle),
            events: proc.events.clone(),
        })
    }

    async fn new_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
    ) -> anyhow::Result<AttachResult> {
        let proc = self.ensure_process(&key);
        if !proc.started.load(Ordering::SeqCst) {
            bail!(
                "opencode: new_session called before attach().start() brought the child online \
                 (agent {key})"
            );
        }

        // Send session/new on the live child; wait for its response.
        let session_id = proc
            .request_new_session(&spec)
            .await
            .context("opencode: session/new request failed")?;

        let handle = OpencodeHandle {
            key,
            local_state: AgentState::Idle,
            spec,
            proc: Arc::clone(&proc),
            preassigned_session_id: Some(session_id),
            bootstraps_process: false,
        };
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
        if !proc.started.load(Ordering::SeqCst) {
            bail!(
                "opencode: resume_session called before attach().start() brought the child online \
                 (agent {key})"
            );
        }

        let resumed_id = proc
            .request_load_session(&spec, &session_id)
            .await
            .context("opencode: session/load request failed")?;

        let handle = OpencodeHandle {
            key,
            local_state: AgentState::Idle,
            spec,
            proc: Arc::clone(&proc),
            preassigned_session_id: Some(resumed_id),
            bootstraps_process: false,
        };
        Ok(AttachResult {
            handle: Box::new(handle),
            events: proc.events.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Pending-request map entries
// ---------------------------------------------------------------------------

/// What an outgoing JSON-RPC id was. Used by the reader to route responses
/// correctly when `acp_protocol::parse_line`'s id-based classification would
/// misclassify them (any id ≥ 3 looks like a prompt response to the parser).
enum PendingKind {
    /// id 1 — the one-shot handshake initialize.
    Initialize,
    /// The inline handshake `session/new` (id 2) or any later one spawned via
    /// `new_session`. The oneshot delivers the minted session id back to the
    /// caller, or an error if the runtime failed.
    NewSession {
        responder: oneshot::Sender<anyhow::Result<String>>,
    },
    /// `session/load` for resuming a caller-supplied session id. Included
    /// here so we can echo the id back through the oneshot even when the
    /// runtime's response body omits `sessionId` (some do).
    LoadSession {
        requested_session_id: String,
        responder: oneshot::Sender<anyhow::Result<String>>,
    },
    /// A `session/prompt`. Carries enough context for the reader to emit the
    /// correct `Completed` event when the response arrives.
    Prompt { session_id: String, run_id: RunId },
}

// ---------------------------------------------------------------------------
// Shared reader state
// ---------------------------------------------------------------------------

/// Per-session live state tracked by the reader. One entry per `session_id`.
struct SessionRuntimeState {
    /// Active run-id when a prompt is in flight. Cleared on response.
    run_id: Option<RunId>,
    /// Tool-call accumulator is per-session because ids are only unique
    /// within a session; mixing sessions would merge calls incorrectly.
    accumulator: ToolCallAccumulator,
    /// Latest state this session has transitioned into. Mirrors what was
    /// emitted on the shared event stream.
    agent_state: AgentState,
}

impl SessionRuntimeState {
    fn active(session_id: &str) -> Self {
        Self {
            run_id: None,
            accumulator: ToolCallAccumulator::new(),
            agent_state: AgentState::Active {
                session_id: session_id.to_string(),
            },
        }
    }
}

struct SharedReaderState {
    /// Classifier for in-flight JSON-RPC responses. Consulted by the reader
    /// before interpreting a response frame.
    pending_requests: HashMap<u64, PendingKind>,
    /// Per-session live state, keyed by the runtime's `sessionId`.
    sessions: HashMap<String, SessionRuntimeState>,
    /// The initial `attach` session id (from id-2 response), stashed so we
    /// can emit `SessionAttached` + `Active` once it lands. `None` after the
    /// first `SessionResponse` drains it.
    bootstrap_pending_prompt: Option<(String, String)>,
    /// Caller-supplied resume id for the initial handshake (id 2). If
    /// `session/load` omits `sessionId`, we fall back to this.
    bootstrap_requested_session_id: Option<String>,
}

impl SharedReaderState {
    fn new() -> Self {
        Self {
            pending_requests: HashMap::new(),
            sessions: HashMap::new(),
            bootstrap_pending_prompt: None,
            bootstrap_requested_session_id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// OpencodeAgentProcess
// ---------------------------------------------------------------------------

/// Shared runtime process for one agent. Multiple `OpencodeHandle`s may hold
/// an `Arc` to the same process and concurrently drive distinct sessions on
/// it.
pub struct OpencodeAgentProcess {
    /// Agent key this process belongs to. Carried so the reader's event
    /// emissions (wired via `.clone()`) stay consistent with the
    /// `OpencodeHandle::key` they feed.
    #[allow(dead_code)]
    key: AgentKey,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    child: Mutex<Option<std::process::Child>>,
    stdin_tx: Mutex<Option<mpsc::Sender<String>>>,
    shared: Arc<Mutex<SharedReaderState>>,
    /// Next JSON-RPC request id. Starts at 3 because ids 1 (initialize) and
    /// 2 (first session request) are reserved for the handshake.
    next_request_id: AtomicU64,
    reader_handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Flipped to true once `start` has spawned the child and written the
    /// handshake. Gates `new_session` / `resume_session`.
    started: std::sync::atomic::AtomicBool,
}

impl OpencodeAgentProcess {
    fn alloc_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Send a raw JSON-RPC line on the shared stdin. Returns `Err` if the
    /// child is no longer live.
    async fn send_line(&self, line: String) -> anyhow::Result<()> {
        let tx = {
            let guard = self.stdin_tx.lock().unwrap();
            guard.clone()
        };
        let tx = tx.context("opencode: stdin not available — child not started")?;
        tx.send(line).await.context("opencode: stdin channel closed")
    }

    /// Register a pending response classifier under `id`.
    fn register_pending(&self, id: u64, kind: PendingKind) {
        self.shared.lock().unwrap().pending_requests.insert(id, kind);
    }

    /// Send `session/new` and wait for the minted session id.
    async fn request_new_session(&self, spec: &AgentSpec) -> anyhow::Result<String> {
        let id = self.alloc_id();
        let (responder, rx) = oneshot::channel();
        self.register_pending(id, PendingKind::NewSession { responder });

        let params = serde_json::json!({
            "cwd": spec.working_directory,
            "mcpServers": []
        });
        let req = acp_protocol::build_session_new_request(id, params);
        self.send_line(req).await?;

        // Guard against a stuck child: if the runtime never answers, fail
        // loudly rather than hang the caller. 30s matches typical ACP timeouts.
        let res = tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .context("opencode: timed out waiting for session/new response")?
            .context("opencode: session/new responder dropped")?;
        res
    }

    /// Send `session/load` and wait for confirmation, returning the resumed
    /// session id (falling back to the caller-supplied id if the runtime
    /// omits it in the response).
    async fn request_load_session(
        &self,
        spec: &AgentSpec,
        session_id: &str,
    ) -> anyhow::Result<String> {
        let id = self.alloc_id();
        let (responder, rx) = oneshot::channel();
        self.register_pending(
            id,
            PendingKind::LoadSession {
                requested_session_id: session_id.to_string(),
                responder,
            },
        );

        let params = serde_json::json!({
            "cwd": spec.working_directory,
            "mcpServers": []
        });
        let req = acp_protocol::build_session_load_request(id, session_id, params);
        self.send_line(req).await?;

        let res = tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .context("opencode: timed out waiting for session/load response")?
            .context("opencode: session/load responder dropped")?;
        res
    }

    /// Signal the child to exit. Idempotent.
    fn kill_child(&self) {
        let mut guard = self.child.lock().unwrap();
        if let Some(ref mut child) = *guard {
            let pid = child.id();
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        *guard = None;
    }
}

impl Drop for OpencodeAgentProcess {
    fn drop(&mut self) {
        self.kill_child();
        let mut handles = self.reader_handles.lock().unwrap();
        for h in handles.drain(..) {
            h.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// OpencodeHandle
// ---------------------------------------------------------------------------

pub struct OpencodeHandle {
    key: AgentKey,
    /// Local view of this handle's lifecycle. Authoritative state for a
    /// session lives in `proc.shared.sessions[session_id]`; this mirror is
    /// used for synchronous read methods (`session_id`, `state`) without
    /// taking the shared lock.
    local_state: AgentState,
    spec: AgentSpec,
    proc: Arc<OpencodeAgentProcess>,
    /// Set by `new_session` / `resume_session` so `start` knows this handle
    /// is attaching to an already-minted session id on the shared child.
    preassigned_session_id: Option<SessionId>,
    /// True for the handle returned from `attach`; false for ones from
    /// `new_session` / `resume_session`. The bootstrap handle is responsible
    /// for spawning the child and driving the handshake.
    bootstraps_process: bool,
}

impl OpencodeHandle {
    fn emit(&self, event: DriverEvent) {
        let _ = self.proc.event_tx.try_send(event);
    }
}

#[async_trait]
impl AgentSessionHandle for OpencodeHandle {
    fn key(&self) -> &AgentKey {
        &self.key
    }

    fn session_id(&self) -> Option<&str> {
        match &self.local_state {
            AgentState::Active { session_id } => Some(session_id),
            AgentState::PromptInFlight { session_id, .. } => Some(session_id),
            _ => self.preassigned_session_id.as_deref(),
        }
    }

    fn state(&self) -> AgentState {
        self.local_state.clone()
    }

    async fn start(
        &mut self,
        opts: StartOpts,
        init_prompt: Option<PromptReq>,
    ) -> anyhow::Result<()> {
        self.local_state = AgentState::Starting;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Starting,
        });

        if self.bootstraps_process {
            // First-attach path: spawn the child, drive handshake, emit
            // SessionAttached + Active when the id-2 response lands.
            self.start_bootstrap_child(opts, init_prompt).await?;
        } else {
            // new_session / resume_session path: child is already live, our
            // session id was minted before we were handed back to the caller.
            let session_id = self
                .preassigned_session_id
                .clone()
                .context("opencode: handle spawned without preassigned session id")?;

            // Seed per-session runtime state and announce the attach.
            {
                let mut s = self.proc.shared.lock().unwrap();
                s.sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| SessionRuntimeState::active(&session_id));
            }
            self.local_state = AgentState::Active {
                session_id: session_id.clone(),
            };
            self.emit(DriverEvent::SessionAttached {
                key: self.key.clone(),
                session_id: session_id.clone(),
            });
            self.emit(DriverEvent::Lifecycle {
                key: self.key.clone(),
                state: AgentState::Active {
                    session_id: session_id.clone(),
                },
            });

            if let Some(req) = init_prompt {
                self.prompt(req).await?;
            }
        }

        Ok(())
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        let session_id = match &self.local_state {
            AgentState::Active { session_id } => session_id.clone(),
            _ => bail!("cannot prompt: handle not in Active state"),
        };

        let run_id = RunId::new_v4();
        let request_id = self.proc.alloc_id();

        // Record pending classifier + per-session run tracking in one place.
        {
            let mut s = self.proc.shared.lock().unwrap();
            s.pending_requests.insert(
                request_id,
                PendingKind::Prompt {
                    session_id: session_id.clone(),
                    run_id,
                },
            );
            if let Some(sess) = s.sessions.get_mut(&session_id) {
                sess.run_id = Some(run_id);
                sess.agent_state = AgentState::PromptInFlight {
                    run_id,
                    session_id: session_id.clone(),
                };
            }
        }

        self.local_state = AgentState::PromptInFlight {
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
        self.proc.send_line(prompt_req).await?;

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        let cancel_info = match &self.local_state {
            AgentState::PromptInFlight { run_id, session_id } => Some((*run_id, session_id.clone())),
            _ => None,
        };
        if let Some((run_id, session_id)) = cancel_info {
            {
                let mut s = self.proc.shared.lock().unwrap();
                if let Some(sess) = s.sessions.get_mut(&session_id) {
                    sess.run_id = None;
                    sess.agent_state = AgentState::Active {
                        session_id: session_id.clone(),
                    };
                }
            }

            self.emit(DriverEvent::Completed {
                key: self.key.clone(),
                session_id: session_id.clone(),
                run_id,
                result: RunResult {
                    finish_reason: FinishReason::Cancelled,
                },
            });

            self.local_state = AgentState::Active { session_id };
            Ok(CancelOutcome::Aborted)
        } else {
            Ok(CancelOutcome::NotInFlight)
        }
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.local_state, AgentState::Closed) {
            return Ok(());
        }

        // Per-handle close: transition our local state and drop our session
        // entry. The shared child is torn down only when the last handle
        // drops — the `OpencodeAgentProcess::Drop` handler does that. In
        // practice the agent-manager drives close for all handles when
        // shutting an agent down, so this keeps the semantics predictable:
        // `close` on any handle quiesces that session but doesn't force-kill
        // sibling sessions' child.
        if let Some(sid) = self.session_id().map(|s| s.to_string()) {
            self.proc.shared.lock().unwrap().sessions.remove(&sid);
        }

        self.local_state = AgentState::Closed;
        self.emit(DriverEvent::Lifecycle {
            key: self.key.clone(),
            state: AgentState::Closed,
        });

        // For the bootstrap handle, closing implies tearing down the shared
        // process — that handle "owns" the lifecycle. This mirrors the v2
        // single-session semantics the existing tests encode.
        if self.bootstraps_process {
            self.proc.kill_child();
            self.proc.events.close();
            let mut handles = self.proc.reader_handles.lock().unwrap();
            for h in handles.drain(..) {
                h.abort();
            }
        }

        Ok(())
    }
}

impl OpencodeHandle {
    /// Spawn the child, write the handshake, and set up the reader tasks.
    /// Only called on the bootstrap handle returned from `attach`.
    async fn start_bootstrap_child(
        &mut self,
        opts: StartOpts,
        init_prompt: Option<PromptReq>,
    ) -> anyhow::Result<()> {
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

        // Write opencode.json to the working directory.
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

        // Register handshake ids BEFORE writing, so an unexpectedly fast
        // runtime can't land a response before the pending map sees it.
        {
            let mut s = self.proc.shared.lock().unwrap();
            s.pending_requests.insert(1, PendingKind::Initialize);
            // Bootstrap session response uses a sentinel responder; we don't
            // hand the session id back through a oneshot for the first
            // handshake because the bootstrap handle receives it via the
            // emitted SessionAttached event (it's been wiring that up).
            let (responder, _rx) = oneshot::channel();
            if let Some(ref sid) = opts.resume_session_id {
                s.bootstrap_requested_session_id = Some(sid.clone());
                s.pending_requests.insert(
                    2,
                    PendingKind::LoadSession {
                        requested_session_id: sid.clone(),
                        responder,
                    },
                );
            } else {
                s.pending_requests.insert(2, PendingKind::NewSession { responder });
            }
            // Stash the init prompt so the reader can fire it once the
            // session is minted. Key by "(session_id, text)" would require
            // the id — defer by stashing under None.
            if let Some(ref req) = init_prompt {
                s.bootstrap_pending_prompt = Some((String::new(), req.text.clone()));
            }
        }

        // Write handshake synchronously before handing stdin to the async writer.
        let init_req = acp_protocol::build_initialize_request(1);
        writeln!(stdin, "{init_req}").context("failed to write initialize request")?;

        let session_new_params = serde_json::json!({
            "cwd": self.spec.working_directory,
            "mcpServers": []
        });

        let session_req = if let Some(ref sid) = opts.resume_session_id {
            acp_protocol::build_session_load_request(2, sid, session_new_params)
        } else {
            acp_protocol::build_session_new_request(2, session_new_params)
        };
        writeln!(stdin, "{session_req}").context("failed to write session request")?;

        // Stdin writer task. Plumbed through `proc.stdin_tx` so subsequent
        // sessions on this process can write too.
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
        {
            let mut guard = self.proc.stdin_tx.lock().unwrap();
            *guard = Some(stdin_tx.clone());
        }
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
        self.proc
            .reader_handles
            .lock()
            .unwrap()
            .push(stdin_handle);

        // Stdout reader task.
        let key = self.key.clone();
        let event_tx = self.proc.event_tx.clone();
        let shared = self.proc.shared.clone();
        let stdin_tx_for_reader = stdin_tx.clone();
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

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                trace!(line = %line, "opencode stdout");

                // Pre-classify responses by id via the pending map, so
                // session/new responses with id ≥ 3 don't get misrouted as
                // prompt responses by the shared parser.
                let classified = classify_line(&line, &shared);

                dispatch_line(
                    classified,
                    &key,
                    &event_tx,
                    &shared,
                    &stdin_tx_for_reader,
                )
                .await;
            }

            // EOF — runtime exited. Flush every session that had an
            // in-flight run.
            let pending_completions: Vec<(String, RunId)> = {
                let s = shared.lock().unwrap();
                s.sessions
                    .iter()
                    .filter_map(|(sid, st)| st.run_id.map(|r| (sid.clone(), r)))
                    .collect()
            };
            for (sid, run_id) in pending_completions {
                let _ = event_tx.try_send(DriverEvent::Completed {
                    key: key.clone(),
                    session_id: sid,
                    run_id,
                    result: RunResult {
                        finish_reason: FinishReason::TransportClosed,
                    },
                });
            }
            // Clear per-session state and emit a single Closed lifecycle.
            {
                let mut s = shared.lock().unwrap();
                s.sessions.clear();
            }
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Closed,
            });
        });
        self.proc
            .reader_handles
            .lock()
            .unwrap()
            .push(stdout_handle);

        // Stderr reader task.
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
        self.proc
            .reader_handles
            .lock()
            .unwrap()
            .push(stderr_handle);

        {
            let mut guard = self.proc.child.lock().unwrap();
            *guard = Some(child);
        }
        self.proc.started.store(true, Ordering::SeqCst);

        // Defer local_state transition to `Active` until the reader observes
        // the session response. Callers who need the session id block on
        // SessionAttached events through the event stream.
        if let Some(ref sid) = opts.resume_session_id {
            // Pre-populate local mirror optimistically; the reader will
            // confirm by emitting SessionAttached / Active.
            self.local_state = AgentState::Active {
                session_id: sid.clone(),
            };
        }
        // For fresh new_session we stay in Starting until the reader fires
        // SessionAttached. The bootstrap pending prompt is delivered by the
        // reader when the session response arrives.
        let _ = init_prompt; // kept for shape parity — stashed earlier

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reader dispatch — classification and handling
// ---------------------------------------------------------------------------

/// Classified event derived from one line. Distinct from `AcpParsed` so we
/// can override id-based response routing without touching `parse_line`.
enum ClassifiedFrame {
    /// id 1 initialize response. Parsed the same as `AcpParsed::InitializeResponse`.
    Initialize,
    /// `session/new` response. `session_id` is whatever the runtime returned.
    /// `responder` delivers it back to the waiting `new_session` call.
    NewSessionResponse {
        session_id: Option<String>,
        responder: Option<oneshot::Sender<anyhow::Result<String>>>,
    },
    /// `session/load` response.
    LoadSessionResponse {
        session_id: Option<String>,
        requested_session_id: String,
        responder: Option<oneshot::Sender<anyhow::Result<String>>>,
    },
    /// A prompt completed. Carries the routing context we stashed when we
    /// sent the request so we can emit the right `Completed` event.
    PromptResponse { session_id: String, run_id: RunId },
    /// Error tied to a known pending id, with context to build the
    /// correct Failed event.
    PendingError { kind: PendingKind, message: String },
    /// A notification (session/update, session/request_permission) or
    /// something unrecognized. Reused as-is from the parser. Untracked
    /// errors (response with an id not in `pending_requests`) surface here
    /// as `AcpParsed::Error` via the fallback in `classify_line`.
    PassThrough(AcpParsed),
}

/// Strip the pending classifier for this line's id if any, then turn the
/// line into a `ClassifiedFrame`. For non-response frames we delegate to
/// `acp_protocol::parse_line`.
fn classify_line(line: &str, shared: &Arc<Mutex<SharedReaderState>>) -> ClassifiedFrame {
    // Peek at the raw JSON to see if it's a response we need to reclassify.
    let raw: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return ClassifiedFrame::PassThrough(AcpParsed::Unknown),
    };

    let is_response = raw.get("id").is_some()
        && (raw.get("result").is_some() || raw.get("error").is_some());
    if !is_response {
        return ClassifiedFrame::PassThrough(acp_protocol::parse_line(line));
    }

    let Some(id) = raw.get("id").and_then(|v| v.as_u64()) else {
        return ClassifiedFrame::PassThrough(acp_protocol::parse_line(line));
    };

    // Extract the pending classifier. If missing, fall through to the raw
    // parser — unsolicited responses are a protocol violation and we'd
    // rather see them as Unknown than silently drop them.
    let pending = shared.lock().unwrap().pending_requests.remove(&id);
    let Some(kind) = pending else {
        return ClassifiedFrame::PassThrough(acp_protocol::parse_line(line));
    };

    // Handle errors first so we can forward them to the waiting responder.
    if let Some(err) = raw.get("error") {
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown ACP error")
            .to_string();
        return ClassifiedFrame::PendingError { kind, message };
    }

    let session_id = raw
        .get("result")
        .and_then(|r| r.get("sessionId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match kind {
        PendingKind::Initialize => ClassifiedFrame::Initialize,
        PendingKind::NewSession { responder } => ClassifiedFrame::NewSessionResponse {
            session_id,
            responder: Some(responder),
        },
        PendingKind::LoadSession {
            requested_session_id,
            responder,
        } => ClassifiedFrame::LoadSessionResponse {
            session_id,
            requested_session_id,
            responder: Some(responder),
        },
        PendingKind::Prompt { session_id: s, run_id } => ClassifiedFrame::PromptResponse {
            session_id: s,
            run_id,
        },
    }
}

/// Handle a classified line: emit events, respond on oneshots, mutate state.
async fn dispatch_line(
    frame: ClassifiedFrame,
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    stdin_tx: &mpsc::Sender<String>,
) {
    match frame {
        ClassifiedFrame::Initialize => {
            debug!("opencode: initialize response received");
        }

        ClassifiedFrame::NewSessionResponse {
            session_id,
            responder,
        } => {
            // Bootstrap path: no responder (we use a dropped oneshot and the
            // session id flows via the event stream).
            // new_session path: responder is Some and we hand the minted id
            // back.
            let sid = session_id.unwrap_or_else(|| {
                warn!("opencode: session/new response omitted sessionId; synthesizing");
                uuid::Uuid::new_v4().to_string()
            });

            // Seed per-session state.
            let deferred_prompt = {
                let mut s = shared.lock().unwrap();
                s.sessions
                    .entry(sid.clone())
                    .or_insert_with(|| SessionRuntimeState::active(&sid));
                s.bootstrap_pending_prompt.take()
            };

            if let Some(responder) = responder {
                if responder.send(Ok(sid.clone())).is_err() {
                    // Caller dropped. That's okay — we already seeded state.
                }
            }

            // Always announce the attach on the shared event stream so UI
            // consumers see it regardless of which path minted the session.
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

            if let Some((_, prompt_text)) = deferred_prompt {
                let run_id = RunId::new_v4();
                {
                    let mut s = shared.lock().unwrap();
                    if let Some(sess) = s.sessions.get_mut(&sid) {
                        sess.run_id = Some(run_id);
                        sess.agent_state = AgentState::PromptInFlight {
                            run_id,
                            session_id: sid.clone(),
                        };
                    }
                    // Track the deferred prompt id in pending_requests so the
                    // classifier recognizes the eventual response.
                    let prompt_id = 3u64; // id 3 is the conventional initial prompt
                    s.pending_requests.insert(
                        prompt_id,
                        PendingKind::Prompt {
                            session_id: sid.clone(),
                            run_id,
                        },
                    );
                }
                let _ = event_tx.try_send(DriverEvent::Lifecycle {
                    key: key.clone(),
                    state: AgentState::PromptInFlight {
                        run_id,
                        session_id: sid.clone(),
                    },
                });

                let req = acp_protocol::build_session_prompt_request(3, &sid, &prompt_text);
                let _ = stdin_tx.try_send(req);
            }
        }

        ClassifiedFrame::LoadSessionResponse {
            session_id,
            requested_session_id,
            responder,
        } => {
            let sid = session_id.unwrap_or(requested_session_id);
            {
                let mut s = shared.lock().unwrap();
                s.sessions
                    .entry(sid.clone())
                    .or_insert_with(|| SessionRuntimeState::active(&sid));
                s.bootstrap_requested_session_id = None;
            }
            if let Some(responder) = responder {
                let _ = responder.send(Ok(sid.clone()));
            }
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
        }

        ClassifiedFrame::PromptResponse { session_id, run_id } => {
            // Flush any pending tool-call accumulator on the matching session,
            // then emit TurnEnd + Completed.
            let drained: Vec<(Option<String>, String, serde_json::Value)> = {
                let mut s = shared.lock().unwrap();
                if let Some(sess) = s.sessions.get_mut(&session_id) {
                    sess.run_id = None;
                    sess.agent_state = AgentState::Active {
                        session_id: session_id.clone(),
                    };
                    sess.accumulator.drain()
                } else {
                    Vec::new()
                }
            };
            for (_id, name, input) in drained {
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: session_id.clone(),
                    run_id,
                    item: AgentEventItem::ToolCall { name, input },
                });
            }
            let _ = event_tx.try_send(DriverEvent::Output {
                key: key.clone(),
                session_id: session_id.clone(),
                run_id,
                item: AgentEventItem::TurnEnd,
            });
            let _ = event_tx.try_send(DriverEvent::Completed {
                key: key.clone(),
                session_id: session_id.clone(),
                run_id,
                result: RunResult {
                    finish_reason: FinishReason::Natural,
                },
            });
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Active { session_id },
            });
        }

        ClassifiedFrame::PendingError { kind, message } => {
            warn!(message = %message, "opencode: ACP error");
            match kind {
                PendingKind::Initialize => {
                    // Initialize failing is terminal; the EOF path will
                    // emit Closed.
                }
                PendingKind::NewSession { responder } => {
                    let _ = responder.send(Err(anyhow::anyhow!("{message}")));
                }
                PendingKind::LoadSession { responder, .. } => {
                    let _ = responder.send(Err(anyhow::anyhow!("{message}")));
                }
                PendingKind::Prompt { session_id, run_id } => {
                    {
                        let mut s = shared.lock().unwrap();
                        if let Some(sess) = s.sessions.get_mut(&session_id) {
                            sess.run_id = None;
                            sess.agent_state = AgentState::Active {
                                session_id: session_id.clone(),
                            };
                        }
                    }
                    let _ = event_tx.try_send(DriverEvent::Failed {
                        key: key.clone(),
                        session_id,
                        run_id,
                        error: AgentError::RuntimeReported(message),
                    });
                }
            }
        }

        ClassifiedFrame::PassThrough(parsed) => match parsed {
            AcpParsed::InitializeResponse => {
                debug!("opencode: initialize response (untracked)");
            }
            AcpParsed::SessionResponse { .. } | AcpParsed::PromptResponse { .. } => {
                // Shouldn't happen: responses always go through classify_line's
                // pending-map path. Log so drift is visible.
                warn!("opencode: response not matched by pending_requests; dropped");
            }
            AcpParsed::SessionUpdate { items } => {
                handle_session_update(items, key, event_tx, shared).await;
            }
            AcpParsed::PermissionRequested {
                request_id,
                tool_name,
                options,
            } => {
                let option_id = acp_protocol::pick_best_option_id(&options);
                debug!(
                    ?tool_name,
                    request_id, option_id, "opencode: auto-approving permission"
                );
                let response = acp_protocol::build_permission_response_raw(request_id, option_id);
                let _ = stdin_tx.try_send(response);
            }
            AcpParsed::Error { message } => {
                warn!(message = %message, "opencode: ACP error (untracked)");
            }
            AcpParsed::Unknown => {}
        },
    }
}

/// Apply the items from a `session/update` notification to the correct
/// per-session accumulator + event stream. We prefer the runtime-provided
/// `SessionInit` session id when present; otherwise any session that's in
/// PromptInFlight state is a candidate — this mirrors how v1 fell back on a
/// single known session id in the single-session case.
async fn handle_session_update(
    items: Vec<AcpUpdateItem>,
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
) {
    // Determine the target session id for these items. `session/update`
    // frames may carry a top-level sessionId we lost in parsing, so we
    // re-route by looking for any session in PromptInFlight. When multiple
    // sessions have prompts in flight, we prefer the most recently started
    // run; ties are broken arbitrarily (sessions on one agent usually run
    // one prompt at a time in practice).
    let (target_session_id, run_id_opt): (Option<String>, Option<RunId>) = {
        let s = shared.lock().unwrap();
        // Pull any SessionInit item first — if present, it's authoritative.
        let init_sid = items.iter().find_map(|it| match it {
            AcpUpdateItem::SessionInit { session_id } => Some(session_id.clone()),
            _ => None,
        });
        if let Some(sid) = init_sid {
            let run_id = s.sessions.get(&sid).and_then(|st| st.run_id);
            (Some(sid), run_id)
        } else {
            // Fall back to any in-flight session.
            s.sessions
                .iter()
                .find_map(|(sid, st)| st.run_id.map(|r| (Some(sid.clone()), Some(r))))
                .unwrap_or((None, None))
        }
    };

    let (Some(sid), Some(run_id)) = (target_session_id, run_id_opt) else {
        return;
    };

    // Drive per-session accumulator and emissions.
    let mut drained_tool_calls: Vec<(Option<String>, String, serde_json::Value)> = Vec::new();
    for item in items {
        match item {
            AcpUpdateItem::SessionInit { .. } => {
                // Already used above.
            }
            AcpUpdateItem::Thinking { text } => {
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: sid.clone(),
                    run_id,
                    item: AgentEventItem::Thinking { text },
                });
            }
            AcpUpdateItem::Text { text } => {
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: sid.clone(),
                    run_id,
                    item: AgentEventItem::Text { text },
                });
            }
            AcpUpdateItem::ToolCall { id, name, input } => {
                let pending_before: Vec<(Option<String>, String, serde_json::Value)> = {
                    let mut s = shared.lock().unwrap();
                    let acc = s
                        .sessions
                        .get_mut(&sid)
                        .map(|st| st.accumulator.drain())
                        .unwrap_or_default();
                    if let Some(sess) = s.sessions.get_mut(&sid) {
                        sess.accumulator.record_call(id, name, input);
                    }
                    acc
                };
                drained_tool_calls.extend(pending_before);
            }
            AcpUpdateItem::ToolCallUpdate { id, input } => {
                let mut s = shared.lock().unwrap();
                if let Some(sess) = s.sessions.get_mut(&sid) {
                    sess.accumulator.merge_update(id, input);
                }
            }
            AcpUpdateItem::ToolResult { content } => {
                let pending_before: Vec<(Option<String>, String, serde_json::Value)> = {
                    let mut s = shared.lock().unwrap();
                    s.sessions
                        .get_mut(&sid)
                        .map(|st| st.accumulator.drain())
                        .unwrap_or_default()
                };
                drained_tool_calls.extend(pending_before);
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: sid.clone(),
                    run_id,
                    item: AgentEventItem::ToolResult { content },
                });
            }
            AcpUpdateItem::TurnEnd => {
                let pending_before: Vec<(Option<String>, String, serde_json::Value)> = {
                    let mut s = shared.lock().unwrap();
                    s.sessions
                        .get_mut(&sid)
                        .map(|st| st.accumulator.drain())
                        .unwrap_or_default()
                };
                drained_tool_calls.extend(pending_before);
                let _ = event_tx.try_send(DriverEvent::Output {
                    key: key.clone(),
                    session_id: sid.clone(),
                    run_id,
                    item: AgentEventItem::TurnEnd,
                });
            }
        }
    }

    for (_id, name, input) in drained_tool_calls {
        let _ = event_tx.try_send(DriverEvent::Output {
            key: key.clone(),
            session_id: sid.clone(),
            run_id,
            item: AgentEventItem::ToolCall { name, input },
        });
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
        // Unique key: the driver's shared registry is process-global, so
        // re-running this test with the same key would re-bind to a stale
        // `OpencodeAgentProcess` from a previous case.
        let key = format!("opencode-test-attach-{}", uuid::Uuid::new_v4());
        let result = driver.attach(key, test_spec()).await.unwrap();
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

    // -----------------------------------------------------------------------
    // Multi-session unit tests (Phase 0.9 Stage 2)
    //
    // These exercise the in-process plumbing without a real `opencode` binary.
    // We construct a shared `OpencodeAgentProcess` by hand, wire up the stdin
    // channel to a test collector, and drive reader dispatch directly by
    // calling `classify_line` + `dispatch_line`. This mirrors the real reader
    // loop faithfully; the only difference is that no child is spawned.
    // -----------------------------------------------------------------------

    /// Build an `OpencodeAgentProcess` prepped for unit-test dispatch.
    /// Returns (process, stdin_rx, event_rx). The process is marked `started`
    /// so `new_session` / `resume_session` don't error out.
    fn build_test_process(
        key: &str,
    ) -> (
        Arc<OpencodeAgentProcess>,
        mpsc::Receiver<String>,
        tokio::sync::mpsc::Receiver<DriverEvent>,
    ) {
        let (events, event_tx) = EventFanOut::new();
        let event_rx = events.subscribe();
        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(64);
        let proc = Arc::new(OpencodeAgentProcess {
            key: key.to_string(),
            events,
            event_tx,
            child: Mutex::new(None),
            stdin_tx: Mutex::new(Some(stdin_tx)),
            shared: Arc::new(Mutex::new(SharedReaderState::new())),
            next_request_id: AtomicU64::new(3),
            reader_handles: Mutex::new(Vec::new()),
            started: std::sync::atomic::AtomicBool::new(true),
        });
        (proc, stdin_rx, event_rx)
    }

    /// Ping a line through the same code path the reader task uses.
    async fn feed_line(proc: &Arc<OpencodeAgentProcess>, line: &str) {
        let frame = classify_line(line, &proc.shared);
        let stdin_tx = {
            let guard = proc.stdin_tx.lock().unwrap();
            guard.clone().expect("stdin present")
        };
        dispatch_line(
            frame,
            &proc.key,
            &proc.event_tx,
            &proc.shared,
            &stdin_tx,
        )
        .await;
    }

    #[tokio::test]
    async fn new_session_mints_distinct_ids_via_live_child() {
        // Simulate: the bootstrap attach already minted session-1 via id 2.
        // Now call new_session twice — each should send a session/new on the
        // shared stdin and resolve with a fresh id carried on the response.
        let (proc, mut stdin_rx, _event_rx) = build_test_process("agent-1");

        // Drive two new_session calls in parallel: each awaits a oneshot
        // response the test will fulfill by feeding back a response line.
        let proc_a = proc.clone();
        let spec_a = test_spec();
        let new_a =
            tokio::spawn(async move { proc_a.request_new_session(&spec_a).await });
        let proc_b = proc.clone();
        let spec_b = test_spec();
        let new_b =
            tokio::spawn(async move { proc_b.request_new_session(&spec_b).await });

        // Collect the two outgoing session/new requests and extract their ids.
        let line_a = stdin_rx.recv().await.expect("first session/new on stdin");
        let line_b = stdin_rx.recv().await.expect("second session/new on stdin");
        let id_a = serde_json::from_str::<serde_json::Value>(&line_a).unwrap()["id"]
            .as_u64()
            .unwrap();
        let id_b = serde_json::from_str::<serde_json::Value>(&line_b).unwrap()["id"]
            .as_u64()
            .unwrap();
        assert_ne!(id_a, id_b, "two session/new calls must use distinct ids");
        assert!(id_a >= 3 && id_b >= 3, "post-handshake ids must be >= 3");

        // Feed responses back through the reader path.
        let resp_a = format!(
            r#"{{"jsonrpc":"2.0","id":{id_a},"result":{{"sessionId":"sess-A"}}}}"#
        );
        let resp_b = format!(
            r#"{{"jsonrpc":"2.0","id":{id_b},"result":{{"sessionId":"sess-B"}}}}"#
        );
        feed_line(&proc, &resp_a).await;
        feed_line(&proc, &resp_b).await;

        let id_out_a = new_a.await.unwrap().unwrap();
        let id_out_b = new_b.await.unwrap().unwrap();
        assert_eq!(id_out_a, "sess-A");
        assert_eq!(id_out_b, "sess-B");
        assert_ne!(
            id_out_a, id_out_b,
            "new_session calls yield distinct session ids"
        );
    }

    #[tokio::test]
    async fn resume_session_preserves_supplied_id() {
        let (proc, mut stdin_rx, _event_rx) = build_test_process("agent-1");

        let proc_1 = proc.clone();
        let spec = test_spec();
        let resume = tokio::spawn(async move {
            proc_1
                .request_load_session(&spec, "stored-xyz")
                .await
        });

        let line = stdin_rx.recv().await.expect("session/load on stdin");
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        let id = parsed["id"].as_u64().unwrap();
        assert_eq!(parsed["method"], "session/load");
        assert_eq!(parsed["params"]["sessionId"], "stored-xyz");

        // Respond with an empty result (some opencode versions do this),
        // forcing the fallback to the requested id.
        let resp = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#);
        feed_line(&proc, &resp).await;

        let id_out = resume.await.unwrap().unwrap();
        assert_eq!(id_out, "stored-xyz", "load fallback preserves supplied id");
    }

    #[tokio::test]
    async fn child_process_is_reused_across_sessions() {
        // `attach` creates the shared process; repeated `attach` + `new_session`
        // on the same key must hand back the same `Arc<OpencodeAgentProcess>`.
        let driver = OpencodeDriver;
        let key = format!("opencode-test-reuse-{}", uuid::Uuid::new_v4());

        let attach = driver.attach(key.clone(), test_spec()).await.unwrap();
        // Find the underlying process from the global registry.
        let proc1 = {
            let g = agent_instances().lock().unwrap();
            Arc::clone(g.get(&key).expect("registered"))
        };

        // Mark started so new_session doesn't bail on the "child online" guard.
        // We can't actually spawn opencode in tests, but the invariant we
        // care about here is registry identity.
        proc1.started.store(true, Ordering::SeqCst);

        // Pre-wire a stdin_tx so request_new_session can write and we can
        // observe the outgoing request.
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
        *proc1.stdin_tx.lock().unwrap() = Some(stdin_tx);

        // Drive new_session on the existing process via the driver API.
        let driver_for_task = OpencodeDriver;
        let key_for_task = key.clone();
        let new_task = tokio::spawn(async move {
            driver_for_task
                .new_session(key_for_task, test_spec())
                .await
        });

        // Fulfil the session/new response.
        let line = stdin_rx.recv().await.unwrap();
        let id = serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"]
            .as_u64()
            .unwrap();
        let resp =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"sess-reuse"}}}}"#);
        feed_line(&proc1, &resp).await;
        let new_attach = new_task.await.unwrap().unwrap();

        // Second lookup: same process.
        let proc2 = {
            let g = agent_instances().lock().unwrap();
            Arc::clone(g.get(&key).expect("registered"))
        };
        assert!(
            Arc::ptr_eq(&proc1, &proc2),
            "same agent key must map to the same OpencodeAgentProcess"
        );

        // Event stream identity: both attach and new_session results share
        // the same fan-out — and therefore the same underlying child.
        assert!(
            Arc::ptr_eq(&attach.events.inner, &proc1.events.inner),
            "attach.events must share fan-out with the shared process"
        );
        assert!(
            Arc::ptr_eq(&new_attach.events.inner, &proc1.events.inner),
            "new_session.events must share fan-out with the shared process"
        );
    }

    #[tokio::test]
    async fn session_update_events_carry_session_id() {
        // Drive a prompt on session A, observe that its session/update items
        // are emitted with session_id == "sess-A". Then drive another prompt
        // on session B; its events must carry "sess-B".
        let (proc, _stdin_rx, mut event_rx) = build_test_process("agent-multi");

        // Seed two sessions as if new_session had minted them.
        {
            let mut s = proc.shared.lock().unwrap();
            s.sessions
                .insert("sess-A".to_string(), SessionRuntimeState::active("sess-A"));
            s.sessions
                .insert("sess-B".to_string(), SessionRuntimeState::active("sess-B"));
        }

        // Simulate prompt-in-flight on sess-A only.
        let run_a = RunId::new_v4();
        {
            let mut s = proc.shared.lock().unwrap();
            let sess = s.sessions.get_mut("sess-A").unwrap();
            sess.run_id = Some(run_a);
            sess.agent_state = AgentState::PromptInFlight {
                run_id: run_a,
                session_id: "sess-A".to_string(),
            };
        }

        // Drive a session/update carrying an agent_message_chunk. Route by
        // SessionInit item so handle_session_update picks sess-A deterministically.
        let update = r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","sessionId":"sess-A","content":{"type":"text","text":"hi from A"}}}}"#;
        // Pre-seed a SessionInit item for deterministic routing. We simulate
        // the reader seeing the SessionInit by crafting an AcpParsed::SessionUpdate
        // with both SessionInit + Text:
        let items = vec![
            AcpUpdateItem::SessionInit {
                session_id: "sess-A".to_string(),
            },
            AcpUpdateItem::Text {
                text: "hi from A".to_string(),
            },
        ];
        handle_session_update(items, &proc.key, &proc.event_tx, &proc.shared).await;
        let _ = update; // kept to document the shape; not fed through parse_line here

        // Drain events — expect one Output(Text) with session_id sess-A.
        let ev = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("event arrived")
            .expect("channel open");
        match ev {
            DriverEvent::Output {
                session_id,
                item: AgentEventItem::Text { text },
                ..
            } => {
                assert_eq!(session_id, "sess-A");
                assert_eq!(text, "hi from A");
            }
            other => panic!("expected Output(Text) for sess-A, got {other:?}"),
        }

        // Now flip: sess-A returns to Active, sess-B goes PromptInFlight.
        {
            let mut s = proc.shared.lock().unwrap();
            let sa = s.sessions.get_mut("sess-A").unwrap();
            sa.run_id = None;
            sa.agent_state = AgentState::Active {
                session_id: "sess-A".to_string(),
            };
            let run_b = RunId::new_v4();
            let sb = s.sessions.get_mut("sess-B").unwrap();
            sb.run_id = Some(run_b);
            sb.agent_state = AgentState::PromptInFlight {
                run_id: run_b,
                session_id: "sess-B".to_string(),
            };
        }
        let items_b = vec![
            AcpUpdateItem::SessionInit {
                session_id: "sess-B".to_string(),
            },
            AcpUpdateItem::Text {
                text: "hi from B".to_string(),
            },
        ];
        handle_session_update(items_b, &proc.key, &proc.event_tx, &proc.shared).await;

        let ev = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("event B arrived")
            .expect("channel open");
        match ev {
            DriverEvent::Output {
                session_id,
                item: AgentEventItem::Text { text },
                ..
            } => {
                assert_eq!(
                    session_id, "sess-B",
                    "event from sess-B must carry its own session id"
                );
                assert_eq!(text, "hi from B");
            }
            other => panic!("expected Output(Text) for sess-B, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn new_session_before_child_started_errors_loudly() {
        // Guard: new_session without a live child (attach.start() wasn't
        // called) should bail with an actionable message, not hang.
        let driver = OpencodeDriver;
        let key = format!("opencode-test-no-child-{}", uuid::Uuid::new_v4());
        let err = match driver.new_session(key, test_spec()).await {
            Ok(_) => panic!("new_session should fail before start"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("before attach"), "got: {msg}");
    }
}
