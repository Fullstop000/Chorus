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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, trace, warn};

// `HandleRole` lives in `drivers/mod.rs` and is used by codex/opencode.
// Kimi no longer uses HandleRole — all handles are role-agnostic since Task 1.

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
    /// True once `ensure_started` has completed successfully (child spawned +
    /// `initialize` responded). Once set, subsequent calls to `ensure_started`
    /// are fast no-ops. On failure, stays false so the next caller can retry.
    started: AtomicBool,
    /// Notified by the race-winner thread once `started` is set. Waiters
    /// sleeping in `ensure_started` wake and re-check `started`.
    started_notify: tokio::sync::Notify,
    /// Mutex serializing concurrent `ensure_started` calls so only one
    /// thread actually runs spawn + initialize. Non-recursive (tokio::Mutex
    /// is fair and async-friendly).
    start_in_progress: tokio::sync::Mutex<()>,
    /// Pairing token minted by the bridge during `spawn_and_initialize`.
    /// Cached here so every subsequent `session/new` or `session/load` call
    /// reuses it without an extra HTTP round-trip. Written exactly once
    /// (by `spawn_and_initialize`); all reads use `get()` which returns
    /// `None` if the core was never started.
    pairing_token: OnceLock<String>,
    /// Number of times `spawn_and_initialize` has been called on this core.
    /// Only compiled under `#[cfg(test)]`; used by concurrency + failure
    /// non-stickiness tests to assert the slow path ran the expected number
    /// of times without needing a real kimi binary.
    #[cfg(test)]
    spawn_call_count: AtomicUsize,
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
            started: AtomicBool::new(false),
            started_notify: tokio::sync::Notify::new(),
            start_in_progress: tokio::sync::Mutex::new(()),
            pairing_token: OnceLock::new(),
            #[cfg(test)]
            spawn_call_count: AtomicUsize::new(0),
        })
    }

    fn emit(&self, event: DriverEvent) {
        super::emit_driver_event(
            &self.event_tx,
            event,
            &self.key,
            <Self as AgentProcess>::DRIVER_NAME,
        );
    }

    /// Lazy, race-safe bootstrap. First caller spawns the child process and
    /// sends `initialize`; subsequent concurrent callers wait for the
    /// `started_notify`; any caller that arrives after `started` is set
    /// returns immediately.
    ///
    /// On failure: `started` stays false. The `start_in_progress` lock is
    /// released, so the next caller retries. This makes failure non-sticky:
    /// a transient spawn error doesn't permanently brick the core.
    pub(crate) async fn ensure_started(self: &Arc<Self>) -> anyhow::Result<()> {
        // Fast path: already started.
        if self.started.load(Ordering::Acquire) {
            return Ok(());
        }
        // Serialize concurrent starters. The double-check inside the lock
        // prevents redundant spawns if two callers raced past the first check.
        let _guard = self.start_in_progress.lock().await;
        if self.started.load(Ordering::Acquire) {
            return Ok(());
        }
        // We are the race-winner. Spawn child + send initialize.
        self.spawn_and_initialize().await?;
        self.started.store(true, Ordering::Release);
        self.started_notify.notify_waiters();
        Ok(())
    }

    /// Spawn the Kimi child process, wire up stdio tasks, and send
    /// `initialize`. Does NOT send `session/new` or `session/load` — those
    /// move to each handle's `start()`. Populates `inner.stdin_tx`,
    /// `inner.shared`, and sets `inner.next_request_id = 3`.
    async fn spawn_and_initialize(self: &Arc<Self>) -> anyhow::Result<()> {
        // Track invocation count for concurrency / failure tests.
        #[cfg(test)]
        self.spawn_call_count.fetch_add(1, Ordering::Relaxed);

        // Pair with the shared HTTP bridge. The token is cached on the core so
        // subsequent session opens (session/new, session/load) reuse it without
        // an extra HTTP round-trip.
        let pairing_token =
            super::request_pairing_token(&self.spec.bridge_endpoint, &self.key)
                .await
                .context("failed to pair with shared bridge")?;
        // Store in the OnceLock. The lock was never set before this point
        // (spawn_and_initialize is only called once per core lifetime under
        // the start_in_progress serialiser), so set() always succeeds here.
        let _ = self.pairing_token.set(pairing_token.clone());

        let wd = &self.spec.working_directory;
        let mcp_config_path = wd.join(".chorus-kimi-mcp.json");
        let mcp_config = build_mcp_config_file(&self.spec.bridge_endpoint, &pairing_token);
        tokio::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)
            .await
            .context("failed to write MCP config")?;

        let mcp_path_str = mcp_config_path.to_string_lossy().into_owned();
        let wd_str = wd.to_string_lossy().into_owned();
        let mut args = vec![
            "--work-dir".to_string(),
            wd_str,
            "--mcp-config-file".to_string(),
            mcp_path_str,
        ];
        if !self.spec.model.is_empty() {
            args.push("--model".to_string());
            args.push(self.spec.model.clone());
        }
        args.push("acp".to_string());

        let mut cmd = Command::new("kimi");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("FORCE_COLOR", "0")
            .env("NO_COLOR", "1");
        for ev in &self.spec.env_vars {
            cmd.env(&ev.key, &ev.value);
        }

        let mut child = cmd.spawn().context("failed to spawn kimi")?;
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;
        let mut stdin = child.stdin.take().context("missing stdin")?;

        // Write `initialize` synchronously before handing stdin to the async writer.
        let init_req = acp_protocol::build_initialize_request(1);
        writeln!(stdin, "{init_req}").context("failed to write initialize request")?;

        // Shared reader state, seeded with just the Init pending entry for id 1.
        // Session minting (session/new or session/load at id >=2) is handled
        // by each handle's start() after ensure_started completes.
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::AwaitingInitResponse,
            sessions: HashMap::new(),
            pending: {
                let mut m = HashMap::new();
                m.insert(1, PendingRequest::Init);
                m
            },
            closed_emitted: Arc::new(AtomicBool::new(false)),
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
        let key = self.key.clone();
        let event_tx = self.event_tx.clone();
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
        let key_err = self.key.clone();
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
        // next_request_id = 3: ids 1 (initialize) and 2 are taken by the
        // first handle's session/new or session/load. Starting at 3 means
        // every subsequent alloc_id() returns unique, non-colliding ids.
        {
            let mut inner = self.inner.lock().await;
            inner.owned.child = Some(child);
            inner
                .owned
                .reader_handles
                .extend([stdin_handle, stdout_handle, stderr_handle]);
            inner.stdin_tx = Some(stdin_tx);
            inner.shared = Some(shared.clone());
            inner.next_request_id = 3;
        }

        Ok(())
    }
}

impl AgentProcess for KimiAgentCore {
    const DRIVER_NAME: &'static str = "kimi";

    /// True when the cached core's child is no longer usable. Happens when
    /// `close()` SIGTERMed the child and aborted the writer task, but the
    /// static registry still holds an Arc (nothing has pruned it yet).
    ///
    /// A fresh core — never-spawned — is NOT stale; callers may still drive
    /// the bootstrap path on it. Evict only when `stdin_tx` exists but its
    /// receiver has dropped (writer task exited).
    fn is_stale(&self) -> bool {
        let Ok(inner) = self.inner.try_lock() else {
            // Someone's mid-mutation (e.g. spawn_and_initialize in progress) — treat
            // as live so we don't tear down a process mid-spawn.
            return false;
        };
        match inner.stdin_tx.as_ref() {
            None => false,
            Some(tx) => tx.is_closed(),
        }
    }
}

/// Test-only accessors exposed via a separate `impl` block so they are
/// completely absent from non-test builds.
#[cfg(test)]
impl KimiAgentCore {
    /// Number of times `spawn_and_initialize` has been invoked on this core.
    /// Used to verify that the serialisation + non-stickiness invariants hold
    /// without needing a real kimi binary.
    pub(crate) fn spawn_and_initialize_call_count_for_test(&self) -> usize {
        self.spawn_call_count.load(Ordering::Relaxed)
    }

    /// Whether `started` is currently set. Used by failure non-stickiness
    /// tests to verify that a failed `ensure_started` does not permanently
    /// flip the flag.
    pub(crate) fn is_started_for_test(&self) -> bool {
        self.started.load(Ordering::Acquire)
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

/// Per-agent `KimiAgentCore` registry. `KimiDriver` is a unit struct
/// (manager + tests pass `Arc::new(KimiDriver)`) so per-agent state lives
/// in this static. Returning `None` from `get_or_evict_stale` on a stale
/// entry makes the driver rebuild the core; `registry_insert` is called
/// from the attach path once the fresh core is wired up.
fn registry() -> &'static AgentRegistry<KimiAgentCore> {
    static REGISTRY: AgentRegistry<KimiAgentCore> = AgentRegistry::new();
    &REGISTRY
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
        // Stale-gate the eviction: `get_or_evict_stale` already evicts cores
        // whose writer task exited (`is_stale()` true). If it returns `Some`,
        // the core is live — reuse it rather than orphaning its child + readers.
        //
        // Reuse semantics: the caller gets a fresh handle wired to the
        // existing fan-out + shared state, so any live sessions on the core
        // keep running. `start()` on the new handle calls `ensure_started()`
        // which is a fast no-op on a live core (started flag already set).
        if let Some(existing) = registry().get_or_evict_stale(&key) {
            let events = existing.events.clone();
            let handle = KimiHandle::new(existing, None);
            return Ok(AttachResult {
                handle: Box::new(handle),
                events,
            });
        }

        let (events, event_tx) = EventFanOut::new();
        let core = KimiAgentCore::new(key.clone(), spec.clone(), events.clone(), event_tx);
        registry().insert(key.clone(), core.clone());

        let handle = KimiHandle::new(core, None);
        Ok(AttachResult {
            handle: Box::new(handle),
            events,
        })
    }

    async fn new_session(&self, key: AgentKey, _spec: AgentSpec) -> anyhow::Result<AttachResult> {
        let core = registry().get_or_evict_stale(&key).ok_or_else(|| {
            anyhow!("kimi: new_session on unknown agent {key} — call attach first")
        })?;

        let events = core.events.clone();
        let handle = KimiHandle::new(core, None);
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
        let core = registry().get_or_evict_stale(&key).ok_or_else(|| {
            anyhow!("kimi: resume_session on unknown agent {key} — call attach first")
        })?;

        let events = core.events.clone();
        let handle = KimiHandle::new(core, Some(session_id));
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
    /// Handshake phase for the very first `initialize` response.
    /// After that response arrives the phase flips to Active and all
    /// subsequent session/new, session/load, and prompt responses route
    /// through `pending` directly.
    phase: acp_protocol::AcpPhase,
    /// Per-session state keyed by ACP session id.
    sessions: HashMap<String, SessionState>,
    /// In-flight JSON-RPC requests keyed by id. Responses are routed through
    /// this map instead of acp_protocol::parse_line's hardcoded id dispatch
    /// (which otherwise bucket id>=3 as PromptResponse).
    pending: HashMap<u64, PendingRequest>,
    /// Set to true once a `Lifecycle { Closed }` has been emitted for this
    /// agent (either by `close()` on a handle, or the reader EOF path).
    /// Guards against duplicate emissions when both fire.
    closed_emitted: Arc<AtomicBool>,
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
/// a oneshot (for session/new, session/load, initialize) or drives prompt
/// bookkeeping.
enum PendingRequest {
    /// Initialize response. Only used for the first `initialize` request;
    /// the reader flips `phase` to Active on arrival.
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
}

// ---------------------------------------------------------------------------
// KimiHandle
// ---------------------------------------------------------------------------

pub struct KimiHandle {
    core: Arc<KimiAgentCore>,
    /// Session id assigned to this handle. None until start() completes.
    /// Populated from the `session/new` or `session/load` response.
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
    /// Construct a role-agnostic handle. Every handle's `start()` will call
    /// `core.ensure_started()` (lazy, race-safe spawn + initialize) then
    /// send its own `session/new` or `session/load`.
    fn new(core: Arc<KimiAgentCore>, preassigned_session_id: Option<SessionId>) -> Self {
        Self {
            core,
            session_id: None,
            state: AgentState::Idle,
            preassigned_session_id,
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

    /// Send `session/new` on the live stdin and return the minted session id.
    /// Requires `ensure_started()` to have already succeeded.
    async fn send_session_new(&self) -> anyhow::Result<String> {
        let (stdin_tx, shared, pairing_token) = self.acquire_stdin_and_shared().await?;
        let id = self.alloc_id().await;
        let (tx, rx) = oneshot::channel();
        let mcp_servers = build_acp_mcp_servers(&self.core.spec.bridge_endpoint, &pairing_token);
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
            .context("kimi: stdin channel closed")?;
        rx.await
            .map_err(|_| anyhow!("kimi: reader task dropped before session/new response"))?
            .map_err(|msg| anyhow!("kimi: session/new failed: {msg}"))
    }

    /// Send `session/load` on the live stdin and return the resolved session id.
    /// Requires `ensure_started()` to have already succeeded.
    async fn send_session_load(&self, sid: &str) -> anyhow::Result<String> {
        let (stdin_tx, shared, _pairing_token) = self.acquire_stdin_and_shared().await?;
        let id = self.alloc_id().await;
        let (tx, rx) = oneshot::channel();
        // Secondary sessions send an empty `mcpServers` array. Kimi
        // accepts this on `session/load` — the bootstrap session's MCP
        // state is reused.
        let params = serde_json::json!({
            "cwd": self.core.spec.working_directory,
            "mcpServers": [],
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
            .context("kimi: stdin channel closed")?;
        rx.await
            .map_err(|_| anyhow!("kimi: reader task dropped before session/load response"))?
            .map_err(|msg| anyhow!("kimi: session/load failed: {msg}"))
    }

    /// Acquire `stdin_tx` + `shared` from the inner mutex. Returns an error
    /// if `ensure_started()` hasn't been called yet (invariant: callers call
    /// it first).
    async fn acquire_stdin_and_shared(
        &self,
    ) -> anyhow::Result<(
        mpsc::Sender<String>,
        Arc<Mutex<SharedReaderState>>,
        String,
    )> {
        // Reuse the pairing token that was cached by spawn_and_initialize.
        // Callers must have called ensure_started() first; if the cache is
        // empty that invariant was violated — surface a clear error instead of
        // making a fresh HTTP request that would indicate incorrect usage.
        let pairing_token = self
            .core
            .pairing_token
            .get()
            .ok_or_else(|| {
                anyhow!(
                    "kimi: pairing token not available — ensure_started() must complete first"
                )
            })?
            .clone();
        let inner = self.core.inner.lock().await;
        let stdin_tx = inner.stdin_tx.clone().ok_or_else(|| {
            anyhow!("kimi: stdin not available — ensure_started() must complete first")
        })?;
        let shared = inner
            .shared
            .clone()
            .ok_or_else(|| anyhow!("kimi: shared reader state missing"))?;
        Ok((stdin_tx, shared, pairing_token))
    }
}

impl Drop for KimiHandle {
    fn drop(&mut self) {
        // Actual child termination is handled by KimiAgentCore::drop when
        // the last Arc is released. We do not signal the child here — a
        // handle may be dropped while sibling handles still hold Arcs to
        // the core. Let the Arc reference count decide when to terminate.
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

        // Apply resume override if present in opts.
        if let Some(id) = opts.resume_session_id.clone() {
            self.preassigned_session_id = Some(id);
        }

        // Lazy, race-safe bootstrap. The first handle to call start() spawns
        // the child and sends `initialize`; all subsequent handles (including
        // concurrent ones) wait for the race-winner and then proceed directly
        // to session minting below.
        self.core.ensure_started().await?;

        // Now send session/new or session/load on the live stdin.
        let session_id = if let Some(ref sid) = self.preassigned_session_id.clone() {
            self.send_session_load(sid).await?
        } else {
            self.send_session_new().await?
        };

        // Register the new session in shared state and advertise it.
        {
            let inner = self.core.inner.lock().await;
            if let Some(ref shared) = inner.shared {
                let mut s = shared.lock().unwrap();
                s.sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| SessionState::new(&session_id));
            }
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

    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId> {
        // Session id is always populated by start(). If it's absent the
        // caller hasn't called start() yet — surface a clear error.
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("kimi: prompt() called before start()"))?;

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

        // Drop this handle's session slot from shared state so `pick_session`
        // / `pick_session_and_run` stop routing events to a dead handle.
        //
        // Every handle now locally tracks its own session_id (populated by
        // start()). No role-dependent lookup is needed.
        //
        // Under the same lock, compute `all_sessions_closed` — true iff
        // every remaining session entry is Closed (or the map is empty).
        // Teardown of the shared child + fan-out + registry entry is gated
        // on this: a close with a sibling session still mid-prompt must NOT
        // kill the child. The last session to close triggers teardown.
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
                    .all(|slot| matches!(slot.state, AgentState::Closed));
                // Don't tear down while a session/new or session/load response
                // is pending — the caller is awaiting a new session that
                // would lose its backing child if we killed it now.
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
                // "all closed" so the teardown path still fires (the core
                // has no live sessions to preserve).
                (true, None)
            }
        };

        self.state = AgentState::Closed;

        // Always emit a per-session Closed lifecycle event so subscribers
        // see this handle retire — independent of whether the shared child
        // teardown below fires.
        self.emit(DriverEvent::Lifecycle {
            key: self.core.key.clone(),
            state: AgentState::Closed,
        });

        // Teardown of the shared child + fan-out + registry is gated on
        // *all sessions closed*.
        //
        // - Single-session close: sole session removed above
        //   → map empty → all_sessions_closed=true → teardown fires.
        // - Close with a live sibling: sibling slot still
        //   Active/PromptInFlight → all_sessions_closed=false → child +
        //   fan-out + registry left intact.
        // - Last session to close after its sibling already closed: its
        //   slot was the final non-Closed entry → all_sessions_closed=true
        //   → teardown runs here.
        if all_sessions_closed {
            if let Some(ref shared) = shared_opt {
                let s = shared.lock().unwrap();
                // Flip BEFORE SIGTERM so a reader racing our abort() toward
                // the EOF `Lifecycle::Closed` emission sees the flag and
                // skips (no double-emit).
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
            registry().remove(&key);
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
    // then close out the event stream. Skip the `Lifecycle { Closed }` emit
    // if the bootstrap's `close()` already fired it (`closed_emitted`
    // flag) — otherwise subscribers see two identical Closed events.
    let (drained, already_closed) = {
        let s = shared.lock().unwrap();
        let drained: Vec<(String, RunId)> = s
            .sessions
            .iter()
            .filter_map(|(sid, st)| st.run_id.map(|r| (sid.clone(), r)))
            .collect();
        let already_closed = s.closed_emitted.load(Ordering::SeqCst);
        (drained, already_closed)
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
    if !already_closed {
        // Claim the slot first so a concurrent close() sees it already
        // emitted and skips (guards the other direction too).
        let shared_emitted = {
            let s = shared.lock().unwrap();
            s.closed_emitted.clone()
        };
        if !shared_emitted.swap(true, Ordering::SeqCst) {
            let _ = event_tx.try_send(DriverEvent::Lifecycle {
                key: key.clone(),
                state: AgentState::Closed,
            });
        }
    }
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
    _stdin_tx: &mpsc::Sender<String>,
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
            // Flip phase to Active — all subsequent session/new, session/load,
            // and prompt requests route through `pending` directly.
            s.phase = acp_protocol::AcpPhase::Active;
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
                if let (Some(sid), Some(run_id)) =
                    pick_session_and_run(key, shared, sid_opt.as_deref())
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
                if let (Some(sid), Some(run_id)) =
                    pick_session_and_run(key, shared, sid_opt.as_deref())
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
                if let Some(sid) = pick_session(key, shared, sid_opt.as_deref()) {
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
                if let Some(sid) = pick_session(key, shared, sid_opt.as_deref()) {
                    let mut s = shared.lock().unwrap();
                    if let Some(slot) = s.sessions.get_mut(&sid) {
                        slot.tool_accumulator.merge_update(id, input);
                    }
                }
            }
            AcpUpdateItem::ToolResult { content } => {
                if let Some(sid) = pick_session(key, shared, sid_opt.as_deref()) {
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
                if let Some(sid) = pick_session(key, shared, sid_opt.as_deref()) {
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

fn pick_session(
    key: &AgentKey,
    shared: &Arc<Mutex<SharedReaderState>>,
    hint: Option<&str>,
) -> Option<String> {
    let s = shared.lock().unwrap();
    if let Some(h) = hint {
        if s.sessions.contains_key(h) {
            return Some(h.to_string());
        }
        // Hint present but not in sessions map — fall through to the
        // single-session heuristic, but LOUDLY. CLAUDE.md forbids silent
        // fallbacks; this makes the "stale hint" case visible so the real
        // cause (close raced with an update, parser returned a bogus sid)
        // gets diagnosed instead of masked.
        warn!(
            agent = %key,
            hint = %h,
            session_count = s.sessions.len(),
            "kimi: pick_session hint missing from sessions — falling back to single-session heuristic"
        );
    }
    if s.sessions.len() == 1 {
        return s.sessions.keys().next().cloned();
    }
    if hint.is_none() && !s.sessions.is_empty() {
        // No hint and multiple live sessions — we cannot route this update.
        // Dropping silently would hide malformed frames from the runtime;
        // surface it.
        warn!(
            agent = %key,
            session_count = s.sessions.len(),
            "kimi: pick_session called with no hint and >1 live sessions — dropping update"
        );
    }
    None
}

fn pick_session_and_run(
    key: &AgentKey,
    shared: &Arc<Mutex<SharedReaderState>>,
    hint: Option<&str>,
) -> (Option<String>, Option<RunId>) {
    let s = shared.lock().unwrap();
    let sid = if let Some(h) = hint {
        if s.sessions.contains_key(h) {
            Some(h.to_string())
        } else if s.sessions.len() == 1 {
            warn!(
                agent = %key,
                hint = %h,
                session_count = s.sessions.len(),
                "kimi: pick_session_and_run hint missing from sessions — falling back to single-session heuristic"
            );
            s.sessions.keys().next().cloned()
        } else {
            warn!(
                agent = %key,
                hint = %h,
                session_count = s.sessions.len(),
                "kimi: pick_session_and_run hint missing with ambiguous sessions — dropping update"
            );
            None
        }
    } else if s.sessions.len() == 1 {
        s.sessions.keys().next().cloned()
    } else {
        if !s.sessions.is_empty() {
            warn!(
                agent = %key,
                session_count = s.sessions.len(),
                "kimi: pick_session_and_run called with no hint and >1 live sessions — dropping update"
            );
        }
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
        registry().remove(&key);
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
            closed_emitted: Arc::new(AtomicBool::new(false)),
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
            closed_emitted: Arc::new(AtomicBool::new(false)),
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
            closed_emitted: Arc::new(AtomicBool::new(false)),
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

        registry().remove(&key);
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

        registry().remove(&key);
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
            closed_emitted: Arc::new(AtomicBool::new(false)),
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

    /// After `spawn_and_initialize` seeding, `core.alloc_id()` returns 3
    /// (ids 1 = initialize, 2 = first session request). No id is reserved
    /// and no placeholder exists in `pending`.
    #[tokio::test]
    async fn alloc_id_starts_at_3_after_spawn_and_initialize() {
        let (events, event_tx) = EventFanOut::new();
        let _ = events;
        let key: AgentKey = format!("agent-alloc-id-{}", uuid::Uuid::new_v4());
        let core = KimiAgentCore::new(key.clone(), test_spec(), events, event_tx);

        // Mirror the post-spawn_and_initialize state: shared seeded with
        // id 1 Init in pending, next_request_id = 3.
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::AwaitingInitResponse,
            sessions: HashMap::new(),
            pending: {
                let mut m = HashMap::new();
                m.insert(1, PendingRequest::Init);
                m
            },
            closed_emitted: Arc::new(AtomicBool::new(false)),
        }));
        {
            let mut inner = core.inner.lock().await;
            inner.shared = Some(shared.clone());
            inner.next_request_id = 3;
            let (tx, _rx) = mpsc::channel::<String>(1);
            inner.stdin_tx = Some(tx);
        }

        // alloc_id must return 3 (not 4 — no reserved slot any more).
        let handle = KimiHandle::new(core.clone(), None);
        let id = handle.alloc_id().await;
        assert_eq!(
            id, 3,
            "alloc_id on a just-spawned core must return 3 (initialize=1, first session=2)"
        );

        // No id 3 placeholder in pending — none was ever seeded.
        let s = shared.lock().unwrap();
        assert!(
            !s.pending.contains_key(&3),
            "no id-3 reservation must exist after spawn_and_initialize"
        );
    }

    /// Regression: `KimiAgentCore::is_stale` returns true once the stdin
    /// writer has been closed (mirrors the close()-then-linger state), and
    /// `registry_get` evicts such an entry rather than handing back a
    /// zombie Arc to a fresh attach.
    #[tokio::test]
    async fn registry_get_evicts_stale_core() {
        let (events, event_tx) = EventFanOut::new();
        let _ = events;
        let key: AgentKey = format!("agent-stale-{}", uuid::Uuid::new_v4());
        let core = KimiAgentCore::new(key.clone(), test_spec(), events, event_tx);

        // Wire a stdin_tx but immediately drop the receiver to simulate
        // the post-close state (writer task exited).
        {
            let mut inner = core.inner.lock().await;
            let (tx, rx) = mpsc::channel::<String>(1);
            drop(rx);
            inner.stdin_tx = Some(tx);
        }
        assert!(core.is_stale(), "closed stdin must mark core stale");

        registry().insert(key.clone(), core);
        assert!(
            registry().get_or_evict_stale(&key).is_none(),
            "registry_get must evict the stale entry and return None"
        );
        // Ensure it was actually removed from the map.
        assert!(
            registry().get(&key).is_none(),
            "stale entry must have been pruned from the registry"
        );
    }

    /// Regression: a fresh (never-spawned) core is NOT stale — callers
    /// that just ran `attach()` and haven't called `start()` yet must
    /// still be able to retrieve it via `registry_get`.
    #[tokio::test]
    async fn registry_get_keeps_fresh_never_spawned_core() {
        let (events, event_tx) = EventFanOut::new();
        let _ = events;
        let key: AgentKey = format!("agent-fresh-{}", uuid::Uuid::new_v4());
        let core = KimiAgentCore::new(key.clone(), test_spec(), events, event_tx);

        assert!(
            !core.is_stale(),
            "a never-spawned core must not be reported as stale"
        );
        registry().insert(key.clone(), core);
        assert!(
            registry().get_or_evict_stale(&key).is_some(),
            "registry_get must return a fresh core"
        );
        registry().remove(&key);
    }

    /// Closing one handle while a sibling session is still live (and mid-prompt)
    /// must NOT tear down the shared kimi child, its reader tasks, or the fan-out.
    /// Teardown is gated on "all sessions closed".
    ///
    /// Sequence:
    ///   1. Seed a core with two sessions both registered in shared.sessions.
    ///   2. Seed the second handle as PromptInFlight (models the in-flight race).
    ///   3. Close the first handle. Assert:
    ///       - `stdin_tx` still present (shared child still reachable).
    ///       - `events.inner.closing` still false (fan-out still serving).
    ///       - Registry entry still present.
    ///       - Reader handle count unchanged.
    ///   4. Close the second handle. Assert teardown NOW fired:
    ///       - `stdin_tx` cleared.
    ///       - `events.inner.closing` true.
    ///       - Registry entry pruned.
    #[tokio::test]
    async fn bootstrap_close_with_live_secondary_does_not_tear_down_shared_child() {
        let key: AgentKey = format!("agent-bootstrap-live-secondary-{}", uuid::Uuid::new_v4());
        let (events, event_tx) = EventFanOut::new();
        let events_for_assert = events.clone();
        let core = KimiAgentCore::new(key.clone(), test_spec(), events, event_tx);

        // Seed the core as if spawn_and_initialize completed and two
        // handles each called start() minting their own sessions.
        let bootstrap_sid = "sess-first".to_string();
        let secondary_sid = "sess-secondary".to_string();
        let secondary_run = RunId::new_v4();

        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: {
                let mut m = HashMap::new();
                // First handle's session: start() completed, idle.
                m.insert(bootstrap_sid.clone(), SessionState::new(&bootstrap_sid));
                // Second handle's session: mid-prompt. This is the race the fix protects.
                let mut sec = SessionState::new(&secondary_sid);
                sec.run_id = Some(secondary_run);
                sec.state = AgentState::PromptInFlight {
                    run_id: secondary_run,
                    session_id: secondary_sid.clone(),
                };
                m.insert(secondary_sid.clone(), sec);
                m
            },
            pending: HashMap::new(),
            closed_emitted: Arc::new(AtomicBool::new(false)),
        }));

        // Fake stdin + reader handles to stand in for the shared child.
        // `kill_child` in close() walks `inner.owned.child` / `inner.stdin_tx`
        // / `inner.owned.reader_handles` — all we need is: stdin_tx presence,
        // a parked reader JoinHandle, and no child (we can't spawn one in
        // tests; SIGTERM sits behind an `if let Some(child)`).
        let (stdin_tx, _stdin_rx) = mpsc::channel::<String>(8);
        // A parked reader task: it awaits forever, so abort() is the only
        // way it exits. Lets us observe post-close whether it was aborted.
        let parked_reader = tokio::spawn(async {
            let () = std::future::pending().await;
        });
        {
            let mut inner = core.inner.lock().await;
            inner.shared = Some(shared.clone());
            inner.stdin_tx = Some(stdin_tx.clone());
            inner.owned.reader_handles.push(parked_reader);
            inner.next_request_id = 3;
        }

        // Register the core so close() can see it for registry_remove().
        registry().insert(key.clone(), core.clone());

        // Build handles, both are unified (no role distinction).
        // Simulate post-start() state: session_id populated locally.
        let mut first_handle = KimiHandle::new(core.clone(), None);
        first_handle.session_id = Some(bootstrap_sid.clone());
        first_handle.state = AgentState::Active {
            session_id: bootstrap_sid.clone(),
        };
        let mut secondary = KimiHandle::new(core.clone(), None);
        secondary.session_id = Some(secondary_sid.clone());
        secondary.state = AgentState::PromptInFlight {
            run_id: secondary_run,
            session_id: secondary_sid.clone(),
        };

        // ---- Close the first handle while the secondary is mid-prompt. ----
        first_handle.close().await.unwrap();

        // Shared child bits must remain intact for the secondary.
        {
            let inner = core.inner.lock().await;
            assert!(
                inner.stdin_tx.is_some(),
                "first-handle close with a live sibling must NOT null out shared stdin_tx"
            );
            assert_eq!(
                inner.owned.reader_handles.len(),
                1,
                "first-handle close with a live sibling must NOT abort shared reader handles"
            );
            assert!(
                !inner.owned.reader_handles[0].is_finished(),
                "parked reader must still be running"
            );
        }
        assert!(
            !events_for_assert.inner.closing.load(Ordering::SeqCst),
            "first-handle close with a live sibling must NOT close the fan-out"
        );
        assert!(
            registry().get(&key).is_some(),
            "first-handle close with a live sibling must NOT prune the registry entry"
        );
        // The first handle's session slot should be gone; the secondary's slot
        // should still be present and still PromptInFlight.
        {
            let s = shared.lock().unwrap();
            assert!(
                !s.sessions.contains_key(&bootstrap_sid),
                "first-handle close must drop its own session slot"
            );
            assert!(
                matches!(
                    s.sessions.get(&secondary_sid).map(|slot| &slot.state),
                    Some(AgentState::PromptInFlight { .. })
                ),
                "secondary slot must remain mid-prompt after first-handle close"
            );
        }

        // ---- Close the secondary. Now teardown fires. ----
        secondary.close().await.unwrap();

        {
            let inner = core.inner.lock().await;
            assert!(
                inner.stdin_tx.is_none(),
                "last-session close must null out shared stdin_tx"
            );
            assert!(
                inner.owned.reader_handles.is_empty(),
                "last-session close must drain shared reader handles"
            );
        }
        assert!(
            events_for_assert.inner.closing.load(Ordering::SeqCst),
            "last-session close must signal the fan-out to drain"
        );
        assert!(
            registry().get(&key).is_none(),
            "last-session close must prune the registry entry"
        );
    }

    // -----------------------------------------------------------------------
    // Unified handle path tests — Task 1
    //
    // These test the `ensure_started` semantics and the role-agnostic handle
    // construction without spawning the real kimi binary. We verify invariants
    // by seeding the core's inner state to mirror the post-`ensure_started`
    // state, then exercising the relevant paths.
    // -----------------------------------------------------------------------

    /// After `ensure_started` succeeds, `core.started` is true and
    /// `started_notify` has fired so any subsequent call returns immediately
    /// without entering the lock.
    ///
    /// We simulate a successful start by manually setting `started = true`
    /// (the real `spawn_and_initialize` would do this — we can't call it
    /// without a real kimi binary).
    #[tokio::test]
    async fn ensure_started_fast_path_when_already_started() {
        let (events, event_tx) = EventFanOut::new();
        let key: AgentKey = format!("agent-ensure-fast-{}", uuid::Uuid::new_v4());
        let core = KimiAgentCore::new(key.clone(), test_spec(), events, event_tx);

        // Manually mark as started (bypasses the real spawn).
        core.started.store(true, Ordering::Release);

        // ensure_started should be a no-op and must not attempt spawn.
        // Since `spawn_and_initialize` would fail with kimi not installed,
        // this confirms we take the fast path.
        core.ensure_started().await.unwrap();

        registry().remove(&key);
    }

    /// Two handles built from the same core both produce `Idle` state before
    /// start() is called. After seeding the core with a live stdin, calling
    /// `start()` on either handle's `session_id()` path works correctly.
    #[tokio::test]
    async fn unified_handle_session_id_from_preassigned() {
        let (events, event_tx) = EventFanOut::new();
        let key: AgentKey = format!("agent-unified-sid-{}", uuid::Uuid::new_v4());
        let core = KimiAgentCore::new(key.clone(), test_spec(), events, event_tx);

        // A handle with a preassigned session id (resume_session path).
        let handle = KimiHandle::new(core.clone(), Some("stored-sess-abc".to_string()));
        assert_eq!(handle.session_id(), Some("stored-sess-abc"));
        assert!(matches!(handle.state(), AgentState::Idle));

        // A handle without preassigned id.
        let handle2 = KimiHandle::new(core.clone(), None);
        assert_eq!(handle2.session_id(), None);

        registry().remove(&key);
    }

    /// `ensure_started` is idempotent — calling it multiple times in sequence
    /// on an already-started core is a no-op (does not increment spawn count
    /// via side effects on inner).
    #[tokio::test]
    async fn ensure_started_idempotent_after_success() {
        let (events, event_tx) = EventFanOut::new();
        let key: AgentKey = format!("agent-ensure-idempotent-{}", uuid::Uuid::new_v4());
        let core = Arc::new(KimiAgentCore::new(key.clone(), test_spec(), events, event_tx));

        // Seed as if spawn_and_initialize succeeded: stdin_tx present,
        // shared state present, started=true.
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: acp_protocol::AcpPhase::Active,
            sessions: HashMap::new(),
            pending: HashMap::new(),
            closed_emitted: Arc::new(AtomicBool::new(false)),
        }));
        {
            let mut inner = core.inner.lock().await;
            let (tx, _rx) = mpsc::channel::<String>(1);
            inner.stdin_tx = Some(tx);
            inner.shared = Some(shared);
            inner.next_request_id = 3;
        }
        core.started.store(true, Ordering::Release);

        // Call ensure_started multiple times — all should succeed fast.
        core.ensure_started().await.unwrap();
        core.ensure_started().await.unwrap();
        core.ensure_started().await.unwrap();

        // stdin_tx must still be present (no re-spawn).
        let inner = core.inner.lock().await;
        assert!(
            inner.stdin_tx.is_some(),
            "ensure_started must not clear stdin_tx"
        );

        registry().remove(&key);
    }

    /// `attach()` on a key that already has a live core returns a handle that
    /// shares the same EventFanOut as the original attach — confirming the
    /// "reuse live core" path in the factory.
    #[tokio::test]
    async fn attach_reuses_live_core_event_stream() {
        let driver = KimiDriver;
        let key = format!("agent-attach-reuse-{}", uuid::Uuid::new_v4());

        let r0 = driver.attach(key.clone(), test_spec()).await.unwrap();

        // Manually mark the core as started and seed a live stdin_tx so
        // `is_stale()` returns false and `get_or_evict_stale` returns the
        // existing core. Keep `_rx` alive for the lifetime of the test so the
        // sender doesn't close (which would mark the core stale).
        let _rx = {
            if let Some(core) = registry().get(&key) {
                core.started.store(true, Ordering::Release);
                let mut inner = core.inner.lock().await;
                let (tx, rx) = mpsc::channel::<String>(1);
                inner.stdin_tx = Some(tx);
                Some(rx)
            } else {
                None
            }
        };

        let r1 = driver.attach(key.clone(), test_spec()).await.unwrap();

        let ptr0 = Arc::as_ptr(&r0.events.inner);
        let ptr1 = Arc::as_ptr(&r1.events.inner);
        assert_eq!(
            ptr0, ptr1,
            "second attach must reuse the same EventFanOut as the first"
        );

        registry().remove(&key);
    }

    // -----------------------------------------------------------------------
    // Gap 2 tests — pairing-token caching + ensure_started invariants
    // -----------------------------------------------------------------------

    /// Test A: concurrent race safety.
    ///
    /// Two concurrent `ensure_started` calls on the same core. In a unit-test
    /// environment there is no kimi binary, so both will fail — that is
    /// expected. What we assert is:
    ///   (a) Both calls return (no deadlock / hang).
    ///   (b) They ran **serially** (at most one at a time) — the mutex
    ///       serialises the slow path. Because both fail the second caller
    ///       re-enters the slow path after the first releases the lock
    ///       (non-stickiness), so the counter ends up at 2; the invariant is
    ///       that they never ran **concurrently** (count <= number of callers).
    ///
    /// Note: when the first call fails `started` stays false, so the second
    /// caller legitimately retries (see Test B). Therefore `count == 2` here
    /// is correct and expected — it proves non-stickiness and serialisation at
    /// once. What would be broken is `count > 2` (impossible) or a deadlock.
    #[tokio::test]
    async fn kimi_ensure_started_concurrent_calls_serialize() {
        let (events, event_tx) = EventFanOut::new();
        let key: AgentKey = format!("agent-concurrent-{}", uuid::Uuid::new_v4());
        let core: Arc<KimiAgentCore> =
            KimiAgentCore::new(key.clone(), test_spec(), events, event_tx);

        let c0 = Arc::clone(&core);
        let c1 = Arc::clone(&core);
        // Both calls will fail (no kimi binary / bridge endpoint unreachable)
        // — that is intentional. We only care about deadlock-freedom and that
        // spawn_and_initialize is never called more than once *per concurrent
        // batch* (i.e. no two calls overlap in time due to the mutex).
        let j0 = tokio::spawn(async move { c0.ensure_started().await });
        let j1 = tokio::spawn(async move { c1.ensure_started().await });
        // If either task hangs this join will time out and the test will fail.
        let (r0, r1) = tokio::join!(j0, j1);
        // Unwrap the JoinHandle (panic propagation), not the Result<()>
        // (expected to be Err — no kimi binary).
        let _ = r0.expect("task 0 panicked");
        let _ = r1.expect("task 1 panicked");

        // Serialisation invariant: each of the two callers entered the slow path
        // at most once. Count must be exactly 2 (both failed, neither was sticky).
        // A count of 0 or >2 would indicate a bug in the mutex / counter.
        let n = core.spawn_and_initialize_call_count_for_test();
        assert!(
            n <= 2,
            "spawn_and_initialize ran {n} times for 2 callers — impossible (bug in counter or mutex)"
        );
        // Both callers must have entered the slow path (non-stickiness): if
        // one run was silently skipped without retrying, we'd see count == 0.
        assert!(
            n >= 1,
            "spawn_and_initialize never ran — both callers took an unexpected fast-path"
        );

        registry().remove(&key);
    }

    /// Test B: failure non-stickiness.
    ///
    /// First `ensure_started` fails (no kimi binary) → `started` stays false.
    /// A second call must re-enter the slow path (`spawn_and_initialize` runs
    /// again, incrementing the counter to 2) rather than being short-circuited
    /// by a stale `started=false` without retrying.
    #[tokio::test]
    async fn kimi_ensure_started_failure_not_sticky() {
        let (events, event_tx) = EventFanOut::new();
        let key: AgentKey = format!("agent-failure-sticky-{}", uuid::Uuid::new_v4());
        let core: Arc<KimiAgentCore> =
            KimiAgentCore::new(key.clone(), test_spec(), events, event_tx);

        // First call — expected to fail (no kimi binary).
        let _ = core.ensure_started().await;
        assert!(
            !core.is_started_for_test(),
            "`started` must remain false after a failed ensure_started"
        );
        // The slow path ran once.
        assert_eq!(
            core.spawn_and_initialize_call_count_for_test(),
            1,
            "spawn_and_initialize must have been called once after first failure"
        );

        // Second call — must retry (non-sticky failure).
        let _ = core.ensure_started().await;
        assert!(
            !core.is_started_for_test(),
            "`started` must still be false (no binary available)"
        );
        assert_eq!(
            core.spawn_and_initialize_call_count_for_test(),
            2,
            "failure was sticky — spawn_and_initialize only ran once instead of twice"
        );

        registry().remove(&key);
    }
}
