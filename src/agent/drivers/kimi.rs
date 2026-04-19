//! Native v2 driver for the Kimi runtime using ACP protocol.
//!
//! Multi-session: one Kimi child process per agent, N ACP sessions multiplexed
//! through its stdio. The first session is brought online by
//! [`RuntimeDriver::attach`] + [`AgentSessionHandle::start`]; subsequent
//! sessions are minted by [`RuntimeDriver::new_session`] (fresh `session/new`)
//! or [`RuntimeDriver::resume_session`] (`session/load`) on the existing
//! stdin. All sessions share the same [`EventStreamHandle`]; callers route by
//! `session_id` on each emitted [`DriverEvent`].

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, trace, warn};

use crate::agent::AgentRuntime;
use crate::utils::cmd::{command_exists, home_dir, read_file};

use super::acp_protocol::{self, AcpParsed, AcpUpdateItem, ToolCallAccumulator};
use super::*;

// ---------------------------------------------------------------------------
// MCP config construction
// ---------------------------------------------------------------------------

/// Build the `.chorus-kimi-mcp.json` config file contents.
///
/// Produces the remote HTTP MCP shape, connecting the runtime to the shared
/// bridge at `{endpoint}/token/{token}/mcp`. Kimi requires `transport: "http"`
/// alongside `url` — without it, the runtime defaults to stdio and fails to
/// connect. Verified against the format emitted by `kimi mcp add --transport
/// http`.
fn build_mcp_config_file(bridge_endpoint: &str, token: &str) -> serde_json::Value {
    let url = crate::bridge::token_mcp_url(bridge_endpoint, token);
    serde_json::json!({
        "mcpServers": {
            "chat": {
                "url": url,
                "transport": "http"
            }
        }
    })
}

/// Build the `mcpServers` array for the ACP `session/new` inline params.
///
/// Produces the remote HTTP MCP shape. ACP spec for HTTP MCP servers in
/// session/new params requires:
///   - `type: "http"` (NOT `transport: "http"` like Kimi's file config format)
///   - `headers` array (required, can be empty)
///
/// See <https://agentclientprotocol.com/protocol/session-setup> — sending the
/// wrong shape produces ACP "Invalid params" errors.
fn build_acp_mcp_servers(bridge_endpoint: &str, token: &str) -> serde_json::Value {
    let url = crate::bridge::token_mcp_url(bridge_endpoint, token);
    serde_json::json!([{
        "type": "http",
        "name": "chat",
        "url": url,
        "headers": []
    }])
}

// ---------------------------------------------------------------------------
// Per-agent shared core
// ---------------------------------------------------------------------------

/// Per-agent process state. One Kimi child process + stdio bookkeeping lives
/// here, shared by every [`KimiHandle`] (attach + new_session + resume_session)
/// belonging to the same agent key.
///
/// The core is constructed at [`RuntimeDriver::attach`] time (empty, no child
/// yet). [`AgentSessionHandle::start`] on the first handle spawns the child
/// and starts the stdio tasks; later [`RuntimeDriver::new_session`] /
/// [`RuntimeDriver::resume_session`] reuse it.
struct KimiAgentCore {
    key: AgentKey,
    events: EventStreamHandle,
    event_tx: mpsc::Sender<DriverEvent>,
    spec: AgentSpec,
    inner: tokio::sync::Mutex<CoreInner>,
}

/// Inner mutable state guarded by a tokio mutex so we can `await` while
/// holding the lock (specifically: we write to stdin under lock to serialise
/// request ordering and atomically register the pending-response waiter).
struct CoreInner {
    /// Set once start() completes on the first handle. None until then.
    stdin_tx: Option<mpsc::Sender<String>>,
    /// Shared reader state (handshake phase, per-session state, pending-by-id
    /// response routing). Populated by start().
    shared: Option<Arc<Mutex<SharedReaderState>>>,
    /// Monotonic JSON-RPC id allocator. The first init is id 1, first
    /// session/new is id 2, first prompt id 3 — matches v1 defaults for the
    /// warm-up flow. Subsequent allocations (prompts, additional
    /// session/new, session/load) use `alloc_id`.
    next_request_id: u64,
    /// Owned child + reader join handles. Kept here so Drop on the core
    /// terminates the process even if every handle has been dropped.
    owned: OwnedProcess,
}

#[derive(Default)]
struct OwnedProcess {
    child: Option<std::process::Child>,
    reader_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl KimiAgentCore {
    fn new(
        key: AgentKey,
        spec: AgentSpec,
        events: EventStreamHandle,
        event_tx: mpsc::Sender<DriverEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            key,
            events,
            event_tx,
            spec,
            inner: tokio::sync::Mutex::new(CoreInner {
                stdin_tx: None,
                shared: None,
                next_request_id: 1,
                owned: OwnedProcess::default(),
            }),
        })
    }

    fn emit(&self, event: DriverEvent) {
        let _ = self.event_tx.try_send(event);
    }
}

impl Drop for KimiAgentCore {
    fn drop(&mut self) {
        // Best-effort: terminate the child when the core is dropped. The core
        // lives inside Arc so Drop fires only once all handles + the static
        // registry entry have been released.
        // Note: inner is a tokio::Mutex; try_lock is sufficient here — if
        // something else holds it mid-drop we've already lost the game.
        if let Ok(mut inner) = self.inner.try_lock() {
            if let Some(ref mut child) = inner.owned.child {
                let pid = child.id();
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }
            for handle in inner.owned.reader_handles.drain(..) {
                handle.abort();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Static per-process registry
// ---------------------------------------------------------------------------

/// Per-agent `KimiAgentCore` registry. `KimiDriver` is constructed as a unit
/// struct at multiple call sites (manager + tests pass `Arc::new(KimiDriver)`)
/// which precludes storing state on the driver itself. A module-level static
/// map gives us a single source of truth without forcing a signature change.
fn kimi_registry() -> &'static Mutex<HashMap<AgentKey, Arc<KimiAgentCore>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<AgentKey, Arc<KimiAgentCore>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registry_get(key: &AgentKey) -> Option<Arc<KimiAgentCore>> {
    kimi_registry().lock().unwrap().get(key).cloned()
}

fn registry_insert(key: AgentKey, core: Arc<KimiAgentCore>) {
    kimi_registry().lock().unwrap().insert(key, core);
}

fn registry_remove(key: &AgentKey) {
    kimi_registry().lock().unwrap().remove(key);
}

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
        // If an existing core is still registered for this key (e.g. stale
        // attach from a prior run that never closed cleanly), drop it first
        // so the new attach owns a fresh event stream. This matches the
        // existing single-session semantics where each attach() yielded a
        // brand-new `EventFanOut`.
        registry_remove(&key);

        let (events, event_tx) = EventFanOut::new();
        let core = KimiAgentCore::new(key.clone(), spec.clone(), events.clone(), event_tx);
        registry_insert(key.clone(), core.clone());

        let handle = KimiHandle::new_primary(core);
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }

    async fn new_session(&self, key: AgentKey, _spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let core = registry_get(&key).ok_or_else(|| {
            anyhow!("kimi: new_session on unknown agent {key} — call attach first")
        })?;

        let events = core.events.clone();
        let handle = KimiHandle::new_secondary(core);
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }

    async fn resume_session(
        &self,
        key: AgentKey,
        _spec: AgentSpec,
        session_id: SessionId,
    ) -> anyhow::Result<AttachResult> {
        let core = registry_get(&key).ok_or_else(|| {
            anyhow!("kimi: resume_session on unknown agent {key} — call attach first")
        })?;

        let events = core.events.clone();
        let mut handle = KimiHandle::new_secondary(core);
        handle.preassigned_session_id = Some(session_id);
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }
}

// ---------------------------------------------------------------------------
// Shared reader state
// ---------------------------------------------------------------------------

/// Reader-task state. Populated during `start()`, consumed by the stdout task.
struct SharedReaderState {
    /// First-session warm-up phase. Only the initial handshake
    /// (initialize → session/new|load) drives this; later sessions bypass it
    /// by registering directly in `pending`.
    phase: acp_protocol::AcpPhase,
    /// Per-session state keyed by ACP session id.
    sessions: HashMap<String, SessionState>,
    /// In-flight JSON-RPC requests keyed by id. Responses are routed through
    /// this map instead of acp_protocol::parse_line's hardcoded id dispatch
    /// (which otherwise bucket id>=3 as PromptResponse).
    pending: HashMap<u64, PendingRequest>,
    /// For the very first session, we omit a `pending` entry for id 2 so the
    /// existing warm-up flow still works — but we do need to know what the
    /// user wanted (new vs load) and what to do with deferred initial prompt.
    warmup: Option<WarmupState>,
    /// Cached for tests that want to assert against the primary run_id on
    /// a session that hasn't surfaced via shared.sessions yet.
    #[allow(dead_code)]
    last_warmup_session_id: Option<String>,
}

struct WarmupState {
    /// If set, the first session is actually a resume; the warm-up response
    /// may omit sessionId and we fall back to this. Stored on the pending
    /// entry (WarmupSession::expected_session_id) — kept here too so future
    /// diagnostics / UI can surface "resume target" without digging through
    /// the pending map.
    #[allow(dead_code)]
    expected_session_id: Option<String>,
    /// Deferred initial prompt: delivered as `session/prompt` once the first
    /// session is Active.
    pending_prompt: Option<String>,
}

/// Per-session state. Each ACP session has its own lifecycle and tool-call
/// accumulator so interleaved `session/update` notifications from different
/// sessions don't cross-contaminate.
struct SessionState {
    state: AgentState,
    run_id: Option<RunId>,
    tool_accumulator: ToolCallAccumulator,
}

impl SessionState {
    fn new(session_id: &str) -> Self {
        Self {
            state: AgentState::Active {
                session_id: session_id.to_string(),
            },
            run_id: None,
            tool_accumulator: ToolCallAccumulator::new(),
        }
    }
}

/// What an in-flight JSON-RPC request is waiting for. When the matching
/// response arrives the reader task looks up the entry and either completes
/// a oneshot (for session/new, session/load) or drives prompt bookkeeping.
enum PendingRequest {
    /// Initialize response. Only used for the first session's warm-up; the
    /// reader flips `phase` to AwaitingSessionResponse on arrival.
    Init,
    /// `session/new` response — carries a oneshot that receives the minted
    /// session id (or an error).
    SessionNew {
        responder: oneshot::Sender<Result<String, String>>,
    },
    /// `session/load` response — carries the id the caller requested (to fall
    /// back to if Kimi omits sessionId) plus the responder.
    SessionLoad {
        expected_session_id: String,
        responder: oneshot::Sender<Result<String, String>>,
    },
    /// `session/prompt` response. On arrival we flush the session's
    /// tool-call accumulator, emit TurnEnd + Completed, and flip the
    /// session's state back to Active.
    Prompt { session_id: String, run_id: RunId },
    /// First-session warm-up path: session/new|load that the existing start()
    /// flow sent synchronously without a oneshot (id is hardcoded to 2). Same
    /// outcomes as SessionNew/SessionLoad but drives reader state directly.
    WarmupSession {
        is_load: bool,
        expected_session_id: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// KimiHandle
// ---------------------------------------------------------------------------

pub struct KimiHandle {
    core: Arc<KimiAgentCore>,
    /// True for the handle returned by `attach()` — it owns the child process
    /// lifecycle (spawn on start, terminate on close). Secondary handles
    /// (from `new_session`/`resume_session`) share the process and never kill
    /// it.
    is_primary: bool,
    /// Session id assigned to this handle. None until start() completes.
    /// Secondary handles' `start()` populates this from the
    /// `session/new`/`session/load` response.
    session_id: Option<SessionId>,
    /// Lifecycle mirror for `state()` calls that don't want to take the
    /// shared mutex. Kept in sync with `core.shared.sessions[session_id]`.
    state: AgentState,
    /// For resume paths, the caller supplies this up-front via
    /// `resume_session` or `start(resume_session_id=Some(_))`. The handle's
    /// start() sends `session/load` with this id.
    preassigned_session_id: Option<SessionId>,
}

impl KimiHandle {
    fn new_primary(core: Arc<KimiAgentCore>) -> Self {
        Self {
            core,
            is_primary: true,
            session_id: None,
            state: AgentState::Idle,
            preassigned_session_id: None,
        }
    }

    fn new_secondary(core: Arc<KimiAgentCore>) -> Self {
        Self {
            core,
            is_primary: false,
            session_id: None,
            state: AgentState::Idle,
            preassigned_session_id: None,
        }
    }

    fn emit(&self, event: DriverEvent) {
        self.core.emit(event);
    }

    /// Allocate a new JSON-RPC id from the shared monotonic counter.
    async fn alloc_id(&self) -> u64 {
        let mut inner = self.core.inner.lock().await;
        let id = inner.next_request_id;
        inner.next_request_id += 1;
        id
    }

    /// Spawn the Kimi child process and wire up stdio tasks. First-session
    /// only — called by the primary handle's `start()`.
    async fn spawn_child(
        &mut self,
        opts: &StartOpts,
    ) -> anyhow::Result<(
        Arc<Mutex<SharedReaderState>>,
        String,
        Option<oneshot::Receiver<Result<String, String>>>,
    )> {
        // Pair with the shared HTTP bridge. If pairing fails we surface the
        // error — misconfiguration is loud.
        let pairing_token =
            super::request_pairing_token(&self.core.spec.bridge_endpoint, &self.core.key)
                .await
                .context("failed to pair with shared bridge")?;

        let wd = &self.core.spec.working_directory;
        let mcp_config_path = wd.join(".chorus-kimi-mcp.json");
        let mcp_config = build_mcp_config_file(&self.core.spec.bridge_endpoint, &pairing_token);
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)
            .context("failed to write MCP config")?;

        let mcp_path_str = mcp_config_path.to_string_lossy().into_owned();
        let wd_str = wd.to_string_lossy().into_owned();
        let mut args = vec![
            "--work-dir".to_string(),
            wd_str,
            "--mcp-config-file".to_string(),
            mcp_path_str,
        ];
        if !self.core.spec.model.is_empty() {
            args.push("--model".to_string());
            args.push(self.core.spec.model.clone());
        }
        args.push("acp".to_string());

        let mut cmd = Command::new("kimi");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("FORCE_COLOR", "0")
            .env("NO_COLOR", "1");
        for ev in &self.core.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut child = cmd.spawn().context("failed to spawn kimi")?;
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;
        let mut stdin = child.stdin.take().context("missing stdin")?;

        // Write handshake synchronously before handing stdin to the async writer.
        let init_req = acp_protocol::build_initialize_request(1);
        writeln!(stdin, "{init_req}").context("failed to write initialize request")?;

        let mcp_servers = build_acp_mcp_servers(&self.core.spec.bridge_endpoint, &pairing_token);
        let session_new_params = serde_json::json!({
            "cwd": self.core.spec.working_directory,
            "mcpServers": mcp_servers
        });

        let expected_session_id = opts.resume_session_id.clone();
        let session_req = if let Some(ref sid) = expected_session_id {
            acp_protocol::build_session_load_request(2, sid, session_new_params)
        } else {
            acp_protocol::build_session_new_request(2, session_new_params)
        };
        writeln!(stdin, "{session_req}").context("failed to write session request")?;

        // Shared reader state, seeded with Init + WarmupSession pending entries
        // so the reader task routes the first two responses correctly.
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::AwaitingInitResponse,
            sessions: HashMap::new(),
            pending: {
                let mut m = HashMap::new();
                m.insert(1, PendingRequest::Init);
                m.insert(
                    2,
                    PendingRequest::WarmupSession {
                        is_load: expected_session_id.is_some(),
                        expected_session_id: expected_session_id.clone(),
                    },
                );
                m
            },
            warmup: Some(WarmupState {
                expected_session_id: expected_session_id.clone(),
                pending_prompt: None,
            }),
            last_warmup_session_id: None,
        }));

        // Stdin writer task.
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
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

        // Stdout reader task.
        let key = self.core.key.clone();
        let event_tx = self.core.event_tx.clone();
        let shared_for_reader = shared.clone();
        let stdin_tx_for_reader = stdin_tx.clone();
        let stdout_handle = tokio::spawn(async move {
            reader_loop(
                key,
                event_tx,
                shared_for_reader,
                stdin_tx_for_reader,
                stdout,
            )
            .await;
        });

        // Stderr reader task.
        let key_err = self.core.key.clone();
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

        // Publish the child + stdio into the shared core.
        {
            let mut inner = self.core.inner.lock().await;
            inner.owned.child = Some(child);
            inner
                .owned
                .reader_handles
                .extend([stdin_handle, stdout_handle, stderr_handle]);
            inner.stdin_tx = Some(stdin_tx);
            inner.shared = Some(shared.clone());
            // Next id is 3 — used for the first prompt. alloc_id starts from
            // 3 so follow-up prompts / session ops don't collide with warm-up.
            inner.next_request_id = 3;
        }

        // For the warm-up flow we don't get a oneshot — the reader task
        // drives the `WarmupSession` routing directly. We return None to let
        // start() know it should not .await a responder.
        Ok((shared, "<warmup>".to_string(), None))
    }
}

impl Drop for KimiHandle {
    fn drop(&mut self) {
        // Only the primary handle's Drop needs to intervene — and even then
        // only to unregister from the static map. Actual child termination is
        // handled by KimiAgentCore::drop when the last Arc is released.
        //
        // We do NOT signal the child here: a primary handle may be dropped
        // while secondary handles still hold Arcs to the core. Let the Arc
        // reference count decide when to terminate.
    }
}

#[async_trait]
impl AgentSessionHandle for KimiHandle {
    fn key(&self) -> &AgentKey {
        &self.core.key
    }

    fn session_id(&self) -> Option<&str> {
        match &self.state {
            AgentState::Active { session_id } => Some(session_id.as_str()),
            AgentState::PromptInFlight { session_id, .. } => Some(session_id.as_str()),
            _ => self
                .session_id
                .as_deref()
                .or(self.preassigned_session_id.as_deref()),
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
            key: self.core.key.clone(),
            state: AgentState::Starting,
        });

        if self.is_primary {
            // Spawn (or re-use a partially-spawned core). The current code
            // path only supports one primary per core; duplicate attach+start
            // would already have panicked on stdin_tx being Some.
            {
                let inner = self.core.inner.lock().await;
                if inner.stdin_tx.is_some() {
                    drop(inner);
                    bail!("kimi: primary start() called twice on same core");
                }
            }

            let (shared, _tag, _responder) = self.spawn_child(&opts).await?;

            // Stash the deferred initial prompt — the reader task will fire
            // it after warm-up completes.
            if let Some(req) = init_prompt {
                let mut s = shared.lock().unwrap();
                if let Some(ref mut w) = s.warmup {
                    w.pending_prompt = Some(req.text);
                }
            }

            // Primary handle's session id is filled in by the reader task on
            // the WarmupSession response; we don't wait for it here because
            // the existing single-session contract (and the manager) does not
            // .await a lifecycle transition.
            // Mirror: the reader task emits SessionAttached + Active
            // lifecycle events; consumers subscribe before start and observe
            // them there.
            //
            // For the local self.state mirror, leave Starting — the state()
            // accessor reads the authoritative copy from shared in prompt().
            // Resolve preassigned so session_id() returns Some immediately.
            self.preassigned_session_id = opts.resume_session_id.clone();

            Ok(())
        } else {
            // Secondary: send session/new or session/load on the existing
            // stdin. Wait for the response via a oneshot and populate this
            // handle's session_id.

            let (stdin_tx, shared) = {
                let inner = self.core.inner.lock().await;
                let tx = inner.stdin_tx.clone().ok_or_else(|| {
                    anyhow!("kimi: new_session before primary start() spawned the child")
                })?;
                let shared = inner
                    .shared
                    .clone()
                    .ok_or_else(|| anyhow!("kimi: shared reader state missing"))?;
                (tx, shared)
            };

            let id = self.alloc_id().await;
            let (tx, rx) = oneshot::channel();

            let session_req = if let Some(ref sid) = opts
                .resume_session_id
                .clone()
                .or_else(|| self.preassigned_session_id.clone())
            {
                {
                    let mut s = shared.lock().unwrap();
                    s.pending.insert(
                        id,
                        PendingRequest::SessionLoad {
                            expected_session_id: sid.clone(),
                            responder: tx,
                        },
                    );
                }
                let mcp_servers = build_acp_mcp_servers(
                    &self.core.spec.bridge_endpoint,
                    // reuse-any-token: fine — secondary sessions share the
                    // pairing token already written by the primary start().
                    // We cannot re-pair here without a fresh token endpoint,
                    // and the MCP server is already attached to the bridge.
                    // Kimi treats `mcpServers` as per-session connection
                    // config; passing the same URL is a no-op in practice.
                    // If the bridge changes, the primary must be recycled.
                    "", // sentinel — replaced below if needed
                );
                let _ = mcp_servers; // silence unused
                                     // Reuse the primary's MCP config by re-deriving the URL from
                                     // the bridge endpoint. We don't have the pairing token here,
                                     // so pass an empty mcpServers array. Kimi accepts this on
                                     // session/load (it reuses the primary session's MCP state).
                let params = serde_json::json!({
                    "cwd": self.core.spec.working_directory,
                    "mcpServers": [],
                });
                acp_protocol::build_session_load_request(id, sid, params)
            } else {
                {
                    let mut s = shared.lock().unwrap();
                    s.pending
                        .insert(id, PendingRequest::SessionNew { responder: tx });
                }
                let params = serde_json::json!({
                    "cwd": self.core.spec.working_directory,
                    "mcpServers": [],
                });
                acp_protocol::build_session_new_request(id, params)
            };

            stdin_tx
                .send(session_req)
                .await
                .context("kimi: stdin channel closed")?;

            // Await the response. Reader task sends Ok(session_id) or
            // Err(msg) once the matching id arrives.
            let session_id = rx
                .await
                .map_err(|_| anyhow!("kimi: reader task dropped before session response"))?
                .map_err(|msg| anyhow!("kimi: session request failed: {msg}"))?;

            // Register the new session in shared state + advertise it.
            {
                let mut s = shared.lock().unwrap();
                s.sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| SessionState::new(&session_id));
            }

            self.session_id = Some(session_id.clone());
            self.state = AgentState::Active {
                session_id: session_id.clone(),
            };
            self.emit(DriverEvent::SessionAttached {
                key: self.core.key.clone(),
                session_id: session_id.clone(),
            });
            self.emit(DriverEvent::Lifecycle {
                key: self.core.key.clone(),
                state: AgentState::Active {
                    session_id: session_id.clone(),
                },
            });

            // Fire the initial prompt, if supplied.
            if let Some(req) = init_prompt {
                self.prompt(req).await?;
            }

            Ok(())
        }
    }

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        // Session id comes from our local mirror (secondary) or from shared
        // state after warm-up completed (primary).
        let session_id = if let Some(sid) = self.session_id.clone() {
            sid
        } else {
            // Primary handle after warm-up: look up the first active session
            // in shared state. For single-session usage this is the only
            // session that exists.
            let inner = self.core.inner.lock().await;
            let shared = inner
                .shared
                .clone()
                .ok_or_else(|| anyhow!("kimi: prompt before start"))?;
            drop(inner);
            let s = shared.lock().unwrap();
            // Pick the first (and for primary single-session, only) session
            // with an Active-ish state. If the warm-up session id has been
            // populated we use that.
            s.sessions
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| anyhow!("kimi: prompt before any session active"))?
        };

        let run_id = RunId::new_v4();
        let request_id = self.alloc_id().await;

        // Register pending prompt + flip session state to PromptInFlight.
        let (stdin_tx, shared) = {
            let inner = self.core.inner.lock().await;
            let tx = inner
                .stdin_tx
                .clone()
                .ok_or_else(|| anyhow!("kimi: stdin not available — handle not started"))?;
            let shared = inner
                .shared
                .clone()
                .ok_or_else(|| anyhow!("kimi: shared state missing"))?;
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
            slot.state = AgentState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            };
        }

        self.state = AgentState::PromptInFlight {
            run_id,
            session_id: session_id.clone(),
        };
        self.emit(DriverEvent::Lifecycle {
            key: self.core.key.clone(),
            state: AgentState::PromptInFlight {
                run_id,
                session_id: session_id.clone(),
            },
        });

        let prompt_req =
            acp_protocol::build_session_prompt_request(request_id, &session_id, &req.text);
        stdin_tx
            .send(prompt_req)
            .await
            .context("kimi: stdin channel closed")?;

        Ok(run_id)
    }

    async fn cancel(&mut self, _run: RunId) -> anyhow::Result<CancelOutcome> {
        // Authoritative session state lives in shared.sessions keyed by this
        // handle's session id — self.state may lag the reader.
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
                AgentState::PromptInFlight { run_id, session_id } => {
                    let rid = *run_id;
                    let psid = session_id.clone();
                    slot.run_id = None;
                    slot.state = AgentState::Active {
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

        self.state = AgentState::Active { session_id };
        Ok(CancelOutcome::Aborted)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        if matches!(self.state, AgentState::Closed) {
            return Ok(());
        }

        self.state = AgentState::Closed;
        self.emit(DriverEvent::Lifecycle {
            key: self.core.key.clone(),
            state: AgentState::Closed,
        });

        if self.is_primary {
            // Tear down the shared child + reader tasks + unregister from the
            // static map. Secondary handles close() is a no-op at the process
            // level — they just stop emitting.
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
            registry_remove(&key);
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reader loop
// ---------------------------------------------------------------------------

/// Consume kimi's stdout and drive the shared reader state + event emission.
///
/// Splits into:
///  - manual id-lookup dispatch for JSON-RPC RESPONSES (so id>=3 isn't
///    misclassified by acp_protocol::parse_line as PromptResponse when it's
///    actually a session/new response on a multi-session driver)
///  - parse_line for notifications (session/update) and server requests
///    (session/request_permission)
async fn reader_loop(
    key: AgentKey,
    event_tx: mpsc::Sender<DriverEvent>,
    shared: Arc<Mutex<SharedReaderState>>,
    stdin_tx: mpsc::Sender<String>,
    stdout: std::process::ChildStdout,
) {
    let async_stdout = match tokio::process::ChildStdout::from_std(stdout) {
        Ok(s) => s,
        Err(e) => {
            warn!(key = %key, error = %e, "kimi: failed to convert stdout to async");
            return;
        }
    };
    let reader = BufReader::new(async_stdout);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        trace!(line = %line, "kimi stdout");

        // Try to extract id + session id before leaning on parse_line. We
        // need id to route responses; we need sessionId (from params) to
        // route notifications to the right session.
        let raw: Option<Value> = serde_json::from_str(&line).ok();

        // 1) JSON-RPC responses (have `id` + (`result` | `error`)).
        if let Some(ref msg) = raw {
            let is_response = msg.get("id").is_some()
                && (msg.get("result").is_some() || msg.get("error").is_some());
            if is_response {
                handle_response(&key, &event_tx, &shared, &stdin_tx, msg).await;
                continue;
            }
        }

        // 2) Everything else: lean on parse_line for notifications /
        // permission requests / errors.
        let parsed = acp_protocol::parse_line(&line);
        match parsed {
            AcpParsed::InitializeResponse
            | AcpParsed::SessionResponse { .. }
            | AcpParsed::PromptResponse { .. } => {
                // Already handled by handle_response above. If we got here,
                // parse_line happened to match an Unknown that looked like a
                // response but our raw check didn't catch — log and ignore.
                debug!(line = %line, "kimi: response slipped past raw check — ignoring");
            }
            AcpParsed::SessionUpdate { items } => {
                // Route by sessionId extracted from params.
                let session_id = raw
                    .as_ref()
                    .and_then(|m| m.get("params"))
                    .and_then(|p| p.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                handle_session_update(&key, &event_tx, &shared, session_id, items);
            }
            AcpParsed::PermissionRequested {
                request_id,
                tool_name,
                options,
            } => {
                let option_id = acp_protocol::pick_best_option_id(&options);
                debug!(
                    ?tool_name,
                    request_id, option_id, "kimi: auto-approving permission"
                );
                let response = acp_protocol::build_permission_response_raw(request_id, option_id);
                let _ = stdin_tx.try_send(response);
            }
            AcpParsed::Error { message } => {
                // Without an id we can't pick which session — surface as a
                // generic Failed on the first in-flight session we find.
                warn!(message = %message, "kimi: ACP error (unrouted)");
                let mut s = shared.lock().unwrap();
                let target = s
                    .sessions
                    .iter()
                    .find(|(_, st)| matches!(st.state, AgentState::PromptInFlight { .. }))
                    .map(|(sid, st)| (sid.clone(), st.run_id));
                if let Some((sid, Some(run_id))) = target {
                    let slot = s.sessions.get_mut(&sid).unwrap();
                    slot.run_id = None;
                    slot.state = AgentState::Active {
                        session_id: sid.clone(),
                    };
                    let _ = event_tx.try_send(DriverEvent::Failed {
                        key: key.clone(),
                        session_id: sid,
                        run_id,
                        error: AgentError::RuntimeReported(message),
                    });
                }
            }
            AcpParsed::Unknown => {}
        }
    }

    // EOF — runtime exited. Emit TransportClosed for every in-flight run,
    // then close out the event stream.
    let drained: Vec<(String, RunId)> = {
        let s = shared.lock().unwrap();
        s.sessions
            .iter()
            .filter_map(|(sid, st)| st.run_id.map(|r| (sid.clone(), r)))
            .collect()
    };
    for (sid, run_id) in drained {
        let _ = event_tx.try_send(DriverEvent::Completed {
            key: key.clone(),
            session_id: sid,
            run_id,
            result: RunResult {
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
        for st in s.sessions.values_mut() {
            st.state = AgentState::Closed;
        }
    }
}

async fn handle_response(
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    stdin_tx: &mpsc::Sender<String>,
    msg: &Value,
) {
    let id = match msg.get("id").and_then(|v| v.as_u64()) {
        Some(id) => id,
        None => return,
    };
    let error_msg: Option<String> = msg
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            if msg.get("error").is_some() {
                Some("unknown ACP error".to_string())
            } else {
                None
            }
        });

    let pending = shared.lock().unwrap().pending.remove(&id);
    let Some(pending) = pending else {
        debug!(id, "kimi: response for unknown id — ignoring");
        return;
    };

    match pending {
        PendingRequest::Init => {
            let mut s = shared.lock().unwrap();
            s.phase = acp_protocol::AcpPhase::AwaitingSessionResponse;
            debug!("kimi: initialize response received");
        }
        PendingRequest::SessionNew { responder } => {
            if let Some(msg) = error_msg {
                let _ = responder.send(Err(msg));
                return;
            }
            let session_id = msg
                .get("result")
                .and_then(|r| r.get("sessionId"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            match session_id {
                Some(sid) => {
                    let _ = responder.send(Ok(sid));
                }
                None => {
                    let _ =
                        responder.send(Err("session/new response omitted sessionId".to_string()));
                }
            }
        }
        PendingRequest::SessionLoad {
            expected_session_id,
            responder,
        } => {
            if let Some(msg) = error_msg {
                let _ = responder.send(Err(msg));
                return;
            }
            // Kimi's session/load omits sessionId; fall back to what we sent.
            let session_id = msg
                .get("result")
                .and_then(|r| r.get("sessionId"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or(expected_session_id);
            let _ = responder.send(Ok(session_id));
        }
        PendingRequest::WarmupSession {
            is_load,
            expected_session_id,
        } => {
            let _ = is_load; // retained for future diagnostics
            if let Some(emsg) = error_msg {
                warn!(message = %emsg, "kimi: warm-up session response errored");
                return;
            }
            // Primary-path warm-up: phase → Active, install session state,
            // fire deferred prompt if any.
            let session_id = msg
                .get("result")
                .and_then(|r| r.get("sessionId"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(expected_session_id)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

            let deferred_prompt: Option<String> = {
                let mut s = shared.lock().unwrap();
                s.phase = acp_protocol::AcpPhase::Active;
                s.sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| SessionState::new(&session_id));
                s.last_warmup_session_id = Some(session_id.clone());
                s.warmup.as_mut().and_then(|w| w.pending_prompt.take())
            };

            let _ = event_tx.try_send(DriverEvent::SessionAttached {
                key: key.clone(),
                session_id: session_id.clone(),
            });
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Active {
                    session_id: session_id.clone(),
                },
            });

            if let Some(prompt_text) = deferred_prompt {
                // Build + fire the first session/prompt at id 3 (next_request_id).
                let run_id = RunId::new_v4();
                // alloc an id directly from shared.pending count? We don't
                // have the core Arc here. Use the SharedReaderState to peek
                // at next_request_id — but next_request_id lives on the core,
                // not shared. Simplest: hardcode id 3 here since this is the
                // warm-up path where the contract says id 3 = first prompt.
                // The core's next_request_id is advanced to 3 already by
                // spawn_child; any subsequent prompt will .alloc_id(). This
                // means after this first prompt the next handle.prompt() will
                // get id 4, matching the original behaviour.
                let prompt_id = 3u64;
                {
                    let mut s = shared.lock().unwrap();
                    s.pending.insert(
                        prompt_id,
                        PendingRequest::Prompt {
                            session_id: session_id.clone(),
                            run_id,
                        },
                    );
                    if let Some(slot) = s.sessions.get_mut(&session_id) {
                        slot.run_id = Some(run_id);
                        slot.state = AgentState::PromptInFlight {
                            run_id,
                            session_id: session_id.clone(),
                        };
                    }
                }
                let _ = event_tx.try_send(DriverEvent::Lifecycle {
                    key: key.clone(),
                    state: AgentState::PromptInFlight {
                        run_id,
                        session_id: session_id.clone(),
                    },
                });
                let req = acp_protocol::build_session_prompt_request(
                    prompt_id,
                    &session_id,
                    &prompt_text,
                );
                let _ = stdin_tx.try_send(req);
            }
        }
        PendingRequest::Prompt { session_id, run_id } => {
            let drained: Vec<(Option<String>, String, Value)> = {
                let mut s = shared.lock().unwrap();
                if let Some(slot) = s.sessions.get_mut(&session_id) {
                    let drained = slot.tool_accumulator.drain();
                    slot.run_id = None;
                    slot.state = AgentState::Active {
                        session_id: session_id.clone(),
                    };
                    drained
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
                state: AgentState::Active {
                    session_id: session_id.clone(),
                },
            });
        }
    }
}

fn handle_session_update(
    key: &AgentKey,
    event_tx: &mpsc::Sender<DriverEvent>,
    shared: &Arc<Mutex<SharedReaderState>>,
    session_id_hint: Option<String>,
    items: Vec<AcpUpdateItem>,
) {
    for item in items {
        // Determine session id for this item. Normally comes from the
        // notification envelope; the SessionInit variant updates it.
        let mut sid_opt: Option<String> = None;
        {
            let s = shared.lock().unwrap();
            if let Some(ref hint) = session_id_hint {
                if s.sessions.contains_key(hint) {
                    sid_opt = Some(hint.clone());
                }
            }
            // Fallback: if exactly one session is live, use it. Matches the
            // single-session behaviour of the old reader.
            if sid_opt.is_none() && s.sessions.len() == 1 {
                sid_opt = s.sessions.keys().next().cloned();
            }
        }

        match item {
            AcpUpdateItem::SessionInit { session_id } => {
                let mut s = shared.lock().unwrap();
                s.sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| SessionState::new(&session_id));
            }
            AcpUpdateItem::Thinking { text } => {
                if let (Some(sid), Some(run_id)) = pick_session_and_run(shared, sid_opt.as_deref())
                {
                    let _ = event_tx.try_send(DriverEvent::Output {
                        key: key.clone(),
                        session_id: sid,
                        run_id,
                        item: AgentEventItem::Thinking { text },
                    });
                }
            }
            AcpUpdateItem::Text { text } => {
                if let (Some(sid), Some(run_id)) = pick_session_and_run(shared, sid_opt.as_deref())
                {
                    let _ = event_tx.try_send(DriverEvent::Output {
                        key: key.clone(),
                        session_id: sid,
                        run_id,
                        item: AgentEventItem::Text { text },
                    });
                }
            }
            AcpUpdateItem::ToolCall { id, name, input } => {
                if let Some(sid) = pick_session(shared, sid_opt.as_deref()) {
                    let mut s = shared.lock().unwrap();
                    if let Some(slot) = s.sessions.get_mut(&sid) {
                        // Flush any previous pending calls first.
                        let flushed = slot.tool_accumulator.drain();
                        let run_id = slot.run_id;
                        drop(s);
                        if let Some(run_id) = run_id {
                            for (_id, n, inp) in flushed {
                                let _ = event_tx.try_send(DriverEvent::Output {
                                    key: key.clone(),
                                    session_id: sid.clone(),
                                    run_id,
                                    item: AgentEventItem::ToolCall {
                                        name: n,
                                        input: inp,
                                    },
                                });
                            }
                        }
                        let mut s = shared.lock().unwrap();
                        if let Some(slot) = s.sessions.get_mut(&sid) {
                            slot.tool_accumulator.record_call(id, name, input);
                        }
                    }
                }
            }
            AcpUpdateItem::ToolCallUpdate { id, input } => {
                if let Some(sid) = pick_session(shared, sid_opt.as_deref()) {
                    let mut s = shared.lock().unwrap();
                    if let Some(slot) = s.sessions.get_mut(&sid) {
                        slot.tool_accumulator.merge_update(id, input);
                    }
                }
            }
            AcpUpdateItem::ToolResult { content } => {
                if let Some(sid) = pick_session(shared, sid_opt.as_deref()) {
                    let (flushed, run_id) = {
                        let mut s = shared.lock().unwrap();
                        if let Some(slot) = s.sessions.get_mut(&sid) {
                            (slot.tool_accumulator.drain(), slot.run_id)
                        } else {
                            (Vec::new(), None)
                        }
                    };
                    if let Some(run_id) = run_id {
                        for (_id, n, inp) in flushed {
                            let _ = event_tx.try_send(DriverEvent::Output {
                                key: key.clone(),
                                session_id: sid.clone(),
                                run_id,
                                item: AgentEventItem::ToolCall {
                                    name: n,
                                    input: inp,
                                },
                            });
                        }
                        let _ = event_tx.try_send(DriverEvent::Output {
                            key: key.clone(),
                            session_id: sid,
                            run_id,
                            item: AgentEventItem::ToolResult { content },
                        });
                    }
                }
            }
            AcpUpdateItem::TurnEnd => {
                if let Some(sid) = pick_session(shared, sid_opt.as_deref()) {
                    let (flushed, run_id) = {
                        let mut s = shared.lock().unwrap();
                        if let Some(slot) = s.sessions.get_mut(&sid) {
                            (slot.tool_accumulator.drain(), slot.run_id)
                        } else {
                            (Vec::new(), None)
                        }
                    };
                    if let Some(run_id) = run_id {
                        for (_id, n, inp) in flushed {
                            let _ = event_tx.try_send(DriverEvent::Output {
                                key: key.clone(),
                                session_id: sid.clone(),
                                run_id,
                                item: AgentEventItem::ToolCall {
                                    name: n,
                                    input: inp,
                                },
                            });
                        }
                        let _ = event_tx.try_send(DriverEvent::Output {
                            key: key.clone(),
                            session_id: sid,
                            run_id,
                            item: AgentEventItem::TurnEnd,
                        });
                    }
                }
            }
        }
    }
}

fn pick_session(shared: &Arc<Mutex<SharedReaderState>>, hint: Option<&str>) -> Option<String> {
    let s = shared.lock().unwrap();
    if let Some(h) = hint {
        if s.sessions.contains_key(h) {
            return Some(h.to_string());
        }
    }
    if s.sessions.len() == 1 {
        return s.sessions.keys().next().cloned();
    }
    None
}

fn pick_session_and_run(
    shared: &Arc<Mutex<SharedReaderState>>,
    hint: Option<&str>,
) -> (Option<String>, Option<RunId>) {
    let s = shared.lock().unwrap();
    let sid = if let Some(h) = hint {
        if s.sessions.contains_key(h) {
            Some(h.to_string())
        } else if s.sessions.len() == 1 {
            s.sessions.keys().next().cloned()
        } else {
            None
        }
    } else if s.sessions.len() == 1 {
        s.sessions.keys().next().cloned()
    } else {
        None
    };
    let run = sid
        .as_ref()
        .and_then(|id| s.sessions.get(id))
        .and_then(|slot| slot.run_id);
    (sid, run)
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
            display_name: "test-kimi".to_string(),
            description: None,
            system_prompt: None,
            model: "kimi-code/kimi-for-coding".to_string(),
            reasoning_effort: None,
            env_vars: vec![],
            working_directory: PathBuf::from("/fake"),
            bridge_endpoint: "http://127.0.0.1:1".to_string(),
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
        let key = format!("kimi-agent-idle-{}", uuid::Uuid::new_v4());
        let result = driver.attach(key.clone(), test_spec()).await.unwrap();
        assert!(matches!(result.handle.state(), AgentState::Idle));
        registry_remove(&key);
    }

    // ---- build_mcp_config_file tests ----

    #[test]
    fn build_mcp_config_file_http_shape() {
        let config = build_mcp_config_file("http://127.0.0.1:4321", "tok-xyz");
        let chat = &config["mcpServers"]["chat"];
        assert_eq!(chat["url"], "http://127.0.0.1:4321/token/tok-xyz/mcp");
        assert_eq!(chat["transport"], "http");
        assert!(chat.get("command").is_none());
    }

    #[test]
    fn build_mcp_config_file_trims_trailing_slash() {
        let config = build_mcp_config_file("http://127.0.0.1:4321/", "tok-xyz");
        assert_eq!(
            config["mcpServers"]["chat"]["url"],
            "http://127.0.0.1:4321/token/tok-xyz/mcp"
        );
    }

    // ---- build_acp_mcp_servers tests ----

    #[test]
    fn build_acp_mcp_servers_http_shape() {
        let servers = build_acp_mcp_servers("http://127.0.0.1:4321", "tok-xyz");
        let arr = servers.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        let entry = &arr[0];
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["name"], "chat");
        assert_eq!(entry["url"], "http://127.0.0.1:4321/token/tok-xyz/mcp");
        // Headers array is required by ACP spec (can be empty)
        assert!(entry["headers"].is_array());
        assert!(entry.get("command").is_none());
    }

    #[test]
    fn build_acp_mcp_servers_trims_trailing_slash() {
        let servers = build_acp_mcp_servers("http://127.0.0.1:4321/", "tok-xyz");
        let arr = servers.as_array().expect("array");
        assert_eq!(arr[0]["url"], "http://127.0.0.1:4321/token/tok-xyz/mcp");
    }

    // -----------------------------------------------------------------------
    // Multi-session tests — Phase 0.9 Stage 2
    //
    // These avoid spawning the real kimi binary by constructing a Core
    // manually and driving the reader-loop via an in-process stdio pair.
    // We write ACP-shaped lines into a `mpsc::Receiver<String>` that stands
    // in for kimi's stdout, and inspect the event stream + the pending
    // waiters for correctness.
    //
    // Shape:
    //   1. Build a KimiAgentCore + a "virtual" shared state by hand.
    //   2. Invoke `handle_response` directly with synthesised JSON to prove
    //      id-based routing works for >=3-id session responses.
    //   3. Drive handle state transitions via shared-state manipulation.
    // -----------------------------------------------------------------------

    /// Prove that two independent session/new responses on ids allocated
    /// after the warm-up produce two distinct session ids, routed through
    /// the `pending` map (not the parse_line id>=3 bucket).
    #[tokio::test]
    async fn multi_session_pending_dispatch_routes_session_new_at_id_gt_3() {
        let (events, event_tx) = EventFanOut::new();
        // No subscriber needed for this routing test — we only assert on
        // responder channels.
        let _ = events;

        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: HashMap::new(),
            pending: HashMap::new(),
            warmup: None,
            last_warmup_session_id: None,
        }));
        let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);

        // Stage two pending session/new requests with ids 7 and 8.
        let (tx7, rx7) = oneshot::channel();
        let (tx8, rx8) = oneshot::channel();
        {
            let mut s = shared.lock().unwrap();
            s.pending
                .insert(7, PendingRequest::SessionNew { responder: tx7 });
            s.pending
                .insert(8, PendingRequest::SessionNew { responder: tx8 });
        }

        // Feed the reader two responses via handle_response.
        let key: AgentKey = "agent-x".to_string();
        let resp7: Value =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":7,"result":{"sessionId":"sess-alpha"}}"#)
                .unwrap();
        let resp8: Value =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":8,"result":{"sessionId":"sess-beta"}}"#)
                .unwrap();

        handle_response(&key, &event_tx, &shared, &stdin_tx, &resp7).await;
        handle_response(&key, &event_tx, &shared, &stdin_tx, &resp8).await;

        let got7 = timeout(Duration::from_millis(500), rx7)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let got8 = timeout(Duration::from_millis(500), rx8)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(got7, "sess-alpha");
        assert_eq!(got8, "sess-beta");
        assert_ne!(got7, got8);
    }

    /// `session/load` response that omits `sessionId` should fall back to
    /// the expected id supplied in the pending entry — matching kimi's
    /// real wire behaviour.
    #[tokio::test]
    async fn multi_session_session_load_falls_back_to_expected_id() {
        let (events, event_tx) = EventFanOut::new();
        let _ = events;
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: HashMap::new(),
            pending: HashMap::new(),
            warmup: None,
            last_warmup_session_id: None,
        }));
        let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);

        let (tx, rx) = oneshot::channel();
        {
            let mut s = shared.lock().unwrap();
            s.pending.insert(
                9,
                PendingRequest::SessionLoad {
                    expected_session_id: "stored-xyz".to_string(),
                    responder: tx,
                },
            );
        }

        // kimi session/load response: empty result, sessionId absent.
        let resp: Value = serde_json::from_str(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#).unwrap();
        handle_response(&"k".to_string(), &event_tx, &shared, &stdin_tx, &resp).await;

        let got = timeout(Duration::from_millis(500), rx)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(got, "stored-xyz");
    }

    /// Prompt responses route through the pending map by id and carry the
    /// right session id on the emitted Completed event — proving events
    /// from multiple sessions don't cross-contaminate.
    #[tokio::test]
    async fn multi_session_prompt_response_carries_correct_session_id() {
        let (events, event_tx) = EventFanOut::new();
        let mut rx_events = events.subscribe();

        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: HashMap::new(),
            pending: HashMap::new(),
            warmup: None,
            last_warmup_session_id: None,
        }));
        let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);

        // Seed two sessions with in-flight prompts.
        let run_a = RunId::new_v4();
        let run_b = RunId::new_v4();
        {
            let mut s = shared.lock().unwrap();
            s.sessions.insert(
                "sess-A".to_string(),
                SessionState {
                    state: AgentState::PromptInFlight {
                        run_id: run_a,
                        session_id: "sess-A".to_string(),
                    },
                    run_id: Some(run_a),
                    tool_accumulator: ToolCallAccumulator::new(),
                },
            );
            s.sessions.insert(
                "sess-B".to_string(),
                SessionState {
                    state: AgentState::PromptInFlight {
                        run_id: run_b,
                        session_id: "sess-B".to_string(),
                    },
                    run_id: Some(run_b),
                    tool_accumulator: ToolCallAccumulator::new(),
                },
            );
            s.pending.insert(
                10,
                PendingRequest::Prompt {
                    session_id: "sess-A".to_string(),
                    run_id: run_a,
                },
            );
            s.pending.insert(
                11,
                PendingRequest::Prompt {
                    session_id: "sess-B".to_string(),
                    run_id: run_b,
                },
            );
        }

        let key: AgentKey = "agent-y".to_string();
        let r10: Value = serde_json::from_str(r#"{"jsonrpc":"2.0","id":10,"result":{}}"#).unwrap();
        let r11: Value = serde_json::from_str(r#"{"jsonrpc":"2.0","id":11,"result":{}}"#).unwrap();

        handle_response(&key, &event_tx, &shared, &stdin_tx, &r10).await;
        handle_response(&key, &event_tx, &shared, &stdin_tx, &r11).await;

        // Drain until we've seen the two Completed events. Each must carry
        // its session_id.
        let mut completed: std::collections::HashSet<String> = Default::default();
        let deadline = Duration::from_millis(500);
        while completed.len() < 2 {
            let ev = timeout(deadline, rx_events.recv())
                .await
                .expect("timed out waiting for Completed events")
                .expect("stream closed");
            if let DriverEvent::Completed { session_id, .. } = ev {
                completed.insert(session_id);
            }
        }
        assert!(completed.contains("sess-A"));
        assert!(completed.contains("sess-B"));

        // After handling, sessions' run_id should be cleared and state back
        // to Active.
        let s = shared.lock().unwrap();
        assert!(s.sessions.get("sess-A").unwrap().run_id.is_none());
        assert!(matches!(
            s.sessions.get("sess-A").unwrap().state,
            AgentState::Active { .. }
        ));
    }

    /// Driver-level: `new_session` on an unknown agent must error (no prior
    /// attach). Prevents accidental silent process spawn.
    #[tokio::test]
    async fn new_session_errors_without_prior_attach() {
        let driver = KimiDriver;
        let key = format!("agent-no-attach-{}", uuid::Uuid::new_v4());
        // AttachResult doesn't implement Debug, so we can't use
        // `expect_err(...)`. Match manually on the Result instead.
        let err = match driver.new_session(key.clone(), test_spec()).await {
            Err(e) => e,
            Ok(_) => panic!("new_session should error without prior attach"),
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown agent"), "got: {msg}");

        let err = match driver
            .resume_session(key, test_spec(), "sid".to_string())
            .await
        {
            Err(e) => e,
            Ok(_) => panic!("resume_session should error without prior attach"),
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown agent"), "got: {msg}");
    }

    /// Driver-level: after attach(), `new_session` returns a handle that
    /// shares the same event stream (proving the "one process, many
    /// sessions" invariant — we reuse the core registered at attach).
    #[tokio::test]
    async fn new_session_reuses_attached_core() {
        let driver = KimiDriver;
        let key = format!("agent-reuse-{}", uuid::Uuid::new_v4());

        let attach = driver.attach(key.clone(), test_spec()).await.unwrap();
        let new_res = driver.new_session(key.clone(), test_spec()).await.unwrap();

        // EventStreamHandle is Clone via Arc<EventFanOut>. The two handles
        // must share the SAME Arc — verify by pointer equality on the Arc.
        let attach_ptr = Arc::as_ptr(&attach.events.inner);
        let new_ptr = Arc::as_ptr(&new_res.events.inner);
        assert_eq!(
            attach_ptr, new_ptr,
            "new_session must share attach's EventFanOut"
        );

        registry_remove(&key);
    }

    /// Driver-level: `resume_session` preserves the caller-supplied session
    /// id on the returned handle before start() is called. Mirrors fake.rs
    /// `multi_session_resume_session_preserves_supplied_id`.
    #[tokio::test]
    async fn resume_session_preserves_supplied_id_before_start() {
        let driver = KimiDriver;
        let key = format!("agent-resume-{}", uuid::Uuid::new_v4());

        let _attach = driver.attach(key.clone(), test_spec()).await.unwrap();
        let resumed = driver
            .resume_session(key.clone(), test_spec(), "stored-sess-xyz".to_string())
            .await
            .unwrap();

        assert_eq!(resumed.handle.session_id(), Some("stored-sess-xyz"));

        registry_remove(&key);
    }

    /// Regression guard: if a response arrives for an id that's not in the
    /// pending map (e.g. already-handled, or runtime bug), we log + drop
    /// without panicking.
    #[tokio::test]
    async fn handle_response_ignores_unknown_id() {
        let (events, event_tx) = EventFanOut::new();
        let _ = events;
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: HashMap::new(),
            pending: HashMap::new(),
            warmup: None,
            last_warmup_session_id: None,
        }));
        let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);

        let resp: Value =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":999,"result":{}}"#).unwrap();
        handle_response(&"k".to_string(), &event_tx, &shared, &stdin_tx, &resp).await;
        // No panic, no state mutation.
        let s = shared.lock().unwrap();
        assert!(s.pending.is_empty());
        assert!(s.sessions.is_empty());
    }
}
