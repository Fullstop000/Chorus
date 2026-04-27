//! Shared `AcpNativeCore` — one struct per (agent_key, driver) pair.
//!
//! The core owns the spawned child process + stdio bookkeeping. Every
//! [`super::AcpNativeHandle`] for the same agent shares one of these. The
//! first handle's `run()` triggers [`AcpNativeCore::ensure_started`] which
//! spawns the child and sends `initialize`; subsequent handles are
//! serialized behind the same lock and skip straight to `session/new` or
//! `session/load` once the race-winner has finished.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::warn;

use super::super::acp_protocol::{self, AcpPhase};
use super::super::{
    AgentKey, AgentSpec, DriverEvent, EventStreamHandle,
};

use super::reader::reader_loop;
use super::state::{PendingRequest, SharedReaderState};
use super::AcpDriverConfig;

/// Per-agent process state. One child process + stdio bookkeeping lives
/// here, shared by every [`super::AcpNativeHandle`] for the same agent key.
///
/// Constructed in [`super::open_session`] (empty, no child yet). The first
/// handle's `run()` calls [`AcpNativeCore::ensure_started`] which spawns
/// the child and sends `initialize`; subsequent handles' `run()` calls
/// observe `started == true` and skip straight to session minting.
pub(crate) struct AcpNativeCore {
    pub cfg: &'static AcpDriverConfig,
    pub key: AgentKey,
    pub events: EventStreamHandle,
    pub event_tx: mpsc::Sender<DriverEvent>,
    pub spec: Arc<AgentSpec>,
    pub inner: tokio::sync::Mutex<CoreInner>,

    /// True once `ensure_started` has completed successfully (child spawned
    /// and `initialize` responded). Once set, subsequent calls to
    /// `ensure_started` are fast no-ops. On failure, stays false so the
    /// next caller can retry — non-sticky failure.
    pub started: AtomicBool,

    /// Mutex serializing concurrent `ensure_started` calls so only one
    /// thread actually runs spawn + initialize. Non-recursive
    /// (`tokio::Mutex` is fair and async-friendly).
    pub start_in_progress: tokio::sync::Mutex<()>,

    /// Number of times `spawn_and_initialize` has been called on this
    /// core. Test-only; used by concurrency / failure non-stickiness tests
    /// to assert the slow path ran the expected number of times without
    /// needing a real runtime binary.
    #[cfg(test)]
    pub spawn_call_count: std::sync::atomic::AtomicUsize,
}

/// Inner mutable state guarded by a tokio mutex so we can `.await` while
/// holding the lock (specifically: writes to stdin happen under the lock to
/// serialize request ordering and atomically register the pending-response
/// waiter).
pub(crate) struct CoreInner {
    /// Set once `spawn_and_initialize` completes. None until then.
    pub stdin_tx: Option<mpsc::Sender<String>>,
    /// Shared reader state (handshake phase, per-session state,
    /// pending-by-id response routing). Populated by
    /// `spawn_and_initialize`.
    pub shared: Option<Arc<Mutex<SharedReaderState>>>,
    /// Monotonic JSON-RPC id allocator. The first `initialize` is id 1,
    /// the first `session/new` is id 2; allocation continues from
    /// `next_request_id` for every subsequent prompt / session/new /
    /// session/load.
    pub next_request_id: u64,
    /// Owned child + reader join handles. Kept here so `Drop` on the core
    /// terminates the process even if every handle has been dropped.
    pub owned: OwnedProcess,
}

#[derive(Default)]
pub(crate) struct OwnedProcess {
    pub child: Option<std::process::Child>,
    pub reader_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl AcpNativeCore {
    pub fn new(
        cfg: &'static AcpDriverConfig,
        key: AgentKey,
        spec: AgentSpec,
        events: EventStreamHandle,
        event_tx: mpsc::Sender<DriverEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            key,
            events,
            event_tx,
            spec: Arc::new(spec),
            inner: tokio::sync::Mutex::new(CoreInner {
                stdin_tx: None,
                shared: None,
                next_request_id: 1,
                owned: OwnedProcess::default(),
            }),
            started: AtomicBool::new(false),
            start_in_progress: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            spawn_call_count: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub fn emit(&self, event: DriverEvent) {
        super::emit_through(self.cfg, &self.event_tx, event, &self.key);
    }

    /// Lazy, race-safe bootstrap. First caller spawns the child process
    /// and sends `initialize`; subsequent concurrent callers are
    /// serialized by `start_in_progress` and return immediately after the
    /// flag is set.
    ///
    /// On failure: `started` stays false. The `start_in_progress` lock is
    /// released, so the next caller retries. This makes failure
    /// non-sticky: a transient spawn error doesn't permanently brick the
    /// core.
    pub async fn ensure_started(self: &Arc<Self>) -> anyhow::Result<()> {
        if self.started.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = self.start_in_progress.lock().await;
        if self.started.load(Ordering::Acquire) {
            return Ok(());
        }
        self.spawn_and_initialize().await?;
        self.started.store(true, Ordering::Release);
        Ok(())
    }

    /// Spawn the runtime's child process, wire up stdio tasks, and send
    /// `initialize`. Does NOT send `session/new` or `session/load` —
    /// those move to each handle's `run()`. Populates `inner.stdin_tx`,
    /// `inner.shared`, and sets `inner.next_request_id = 3`.
    async fn spawn_and_initialize(self: &Arc<Self>) -> anyhow::Result<()> {
        #[cfg(test)]
        self.spawn_call_count.fetch_add(1, Ordering::Relaxed);

        let spec = self.spec.clone();
        let mut spawned = (self.cfg.spawn_child)(spec, self.key.clone()).await?;

        let stdout = spawned
            .child
            .stdout
            .take()
            .context("missing stdout")?;
        let stderr = spawned
            .child
            .stderr
            .take()
            .context("missing stderr")?;
        let mut stdin = spawned
            .child
            .stdin
            .take()
            .context("missing stdin")?;

        // Write `initialize` synchronously before handing stdin to the
        // async writer task.
        let init_req = acp_protocol::build_initialize_request(1);
        writeln!(stdin, "{init_req}").context("failed to write initialize request")?;

        // Shared reader state, seeded with just the Init pending entry for
        // id 1. Session minting (session/new or session/load at id >= 2)
        // is handled by each handle's `run()` after `ensure_started`
        // completes.
        let initialized_notification = self
            .cfg
            .initialized_notification_payload
            .map(|s| s.to_string());
        let shared = Arc::new(Mutex::new(SharedReaderState {
            phase: AcpPhase::AwaitingInitResponse,
            sessions: HashMap::new(),
            pending: {
                let mut m = HashMap::new();
                m.insert(1, PendingRequest::Init);
                m
            },
            closed_emitted: Arc::new(AtomicBool::new(false)),
            initialized_notification,
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
        let key_for_reader = self.key.clone();
        let event_tx = self.event_tx.clone();
        let shared_for_reader = shared.clone();
        let stdin_tx_for_reader = stdin_tx.clone();
        let driver_name = self.cfg.name;
        let stdout_handle = tokio::spawn(async move {
            reader_loop(
                driver_name,
                key_for_reader,
                event_tx,
                shared_for_reader,
                stdin_tx_for_reader,
                stdout,
            )
            .await;
        });

        // Stderr reader task: log non-empty lines at warn level with
        // driver context.
        let key_err = self.key.clone();
        let driver_name_err = self.cfg.name;
        let stderr_handle = tokio::spawn(async move {
            let stderr_async = match tokio::process::ChildStderr::from_std(stderr) {
                Ok(s) => s,
                Err(e) => {
                    warn!(key = %key_err, driver = driver_name_err, error = %e, "failed to convert stderr to async");
                    return;
                }
            };
            let reader = BufReader::new(stderr_async);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    warn!(key = %key_err, driver = driver_name_err, line = %line, "stderr");
                }
            }
        });

        // Publish the child + stdio into the shared core.
        // next_request_id = 3: ids 1 (initialize) and 2 are taken by the
        // first handle's session/new or session/load. Starting at 3
        // means every subsequent alloc_id() returns unique,
        // non-colliding ids.
        {
            let mut inner = self.inner.lock().await;
            inner.owned.child = Some(spawned.child);
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

    /// True when the cached core's child is no longer usable. Happens when
    /// `close()` SIGTERMed the child and aborted the writer task, but the
    /// per-driver registry still holds an `Arc` (nothing has pruned it
    /// yet).
    ///
    /// A fresh core — never-spawned — is NOT stale; callers may still
    /// drive the bootstrap path on it. Evict only when `stdin_tx` exists
    /// but its receiver has dropped (writer task exited).
    pub(super) fn is_stale_impl(&self) -> bool {
        let Ok(inner) = self.inner.try_lock() else {
            // Someone's mid-mutation (e.g. spawn_and_initialize in
            // progress) — treat as live so we don't tear down a process
            // mid-spawn.
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
impl AcpNativeCore {
    /// Number of times `spawn_and_initialize` has been invoked on this
    /// core. Used to verify that the serialization + non-stickiness
    /// invariants hold without needing a real runtime binary.
    pub(crate) fn spawn_and_initialize_call_count_for_test(&self) -> usize {
        self.spawn_call_count.load(Ordering::Relaxed)
    }

    /// Whether `started` is currently set. Used by failure non-stickiness
    /// tests to verify that a failed `ensure_started` does not
    /// permanently flip the flag.
    #[allow(dead_code)]
    pub(crate) fn is_started_for_test(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }
}

impl Drop for AcpNativeCore {
    fn drop(&mut self) {
        // Best-effort: terminate the child when the core is dropped. The
        // core lives inside `Arc` so `Drop` fires only once all handles +
        // the registry entry have been released. `try_lock` is sufficient
        // here — if something else holds the inner lock mid-drop we've
        // already lost the game.
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
