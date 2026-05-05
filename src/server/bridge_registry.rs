//! Registry of connected bridges, keyed by `machine_id`.
//!
//! Each connected bridge holds an mpsc sender; the platform pushes
//! outbound WS frames (already JSON-encoded as text) onto that sender,
//! and the WS session task forwards them to the socket. Send failures
//! mean the bridge has disconnected — the session task's RAII guard
//! removes the entry on drop, so callers don't have to clean up.
//!
//! Cardinality: a single `machine_id` may have multiple senders
//! transiently when a new connection arrives before the old one's
//! cleanup runs. Slice 2 broadcasts to all of them; slice 3 (or later)
//! supersedes the older connection per the §4 cardinality rule.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tracing::debug;

/// Cap on outbound frames buffered per bridge before drop. Lifecycle and
/// chat frames are small (<1 KB); 256 leaves headroom for normal bursts.
const PER_BRIDGE_BUFFER: usize = 256;

/// Maps `machine_id` to a list of outbound senders (one per active WS
/// session for that machine). Each sender carries pre-encoded JSON
/// strings ready to write to a `Message::Text` frame.
///
/// Also tracks the current `runtime_pid` per `(machine_id, agent_id)` so
/// `agent.state` payloads from a previous instance (the classic
/// stop→start race) can be filtered out — without this, a delayed
/// `crashed` report from the dead pid silently marks the live new
/// instance dead.
#[derive(Default)]
pub struct BridgeRegistry {
    connections: Mutex<HashMap<String, Vec<mpsc::Sender<String>>>>,
    /// `(machine_id, agent_id) → current_runtime_pid`. Set by every
    /// `agent.state{state=started}` event the bridge sends; checked
    /// against the pid carried by every other transition. Cleared
    /// per-machine on bridge disconnect.
    instance_pids: Mutex<HashMap<String, HashMap<String, u32>>>,
    /// Telemetry: count of `agent.state` frames dropped because their
    /// `runtime_pid` doesn't match the current tracker. Test-visible
    /// hook for verifying the filter actually fires.
    stale_state_drops: AtomicUsize,
}

impl BridgeRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a freshly-connected bridge. Returns the receiver the
    /// session task should drain into its WS socket, plus a guard that
    /// removes the registration when dropped.
    pub fn register(self: &Arc<Self>, machine_id: &str) -> (mpsc::Receiver<String>, Registration) {
        let (tx, rx) = mpsc::channel::<String>(PER_BRIDGE_BUFFER);
        {
            let mut conns = self.connections.lock().unwrap();
            conns
                .entry(machine_id.to_string())
                .or_default()
                .push(tx.clone());
        }
        let guard = Registration {
            registry: Arc::clone(self),
            machine_id: machine_id.to_string(),
            sender: tx,
        };
        (rx, guard)
    }

    /// Push a JSON-encoded frame to every connected bridge. Returns the
    /// number of bridges the frame was successfully queued for.
    /// Disconnected senders are skipped silently; their session task's
    /// guard will deregister them.
    pub fn broadcast(&self, frame_text: &str) -> usize {
        let snapshot: Vec<mpsc::Sender<String>> = {
            let conns = self.connections.lock().unwrap();
            conns.values().flatten().cloned().collect()
        };
        let mut delivered = 0;
        for tx in snapshot {
            // try_send: drop the frame if the bridge's queue is full
            // rather than block the caller (the agent CRUD handler).
            if tx.try_send(frame_text.to_string()).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    fn deregister(&self, machine_id: &str, sender: &mpsc::Sender<String>) {
        let was_last_for_machine = {
            let mut conns = self.connections.lock().unwrap();
            if let Some(list) = conns.get_mut(machine_id) {
                list.retain(|tx| !tx.same_channel(sender));
                if list.is_empty() {
                    conns.remove(machine_id);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        // Clear the pid tracker only when the last connection for this
        // machine_id goes away. If a transient second connection just
        // closed (4002 supersede in a future slice), the surviving one
        // still owns those pids.
        if was_last_for_machine {
            self.instance_pids.lock().unwrap().remove(machine_id);
        }
    }

    /// Record the runtime pid the bridge just started for an agent.
    /// Called on every `agent.state{state=started}` event.
    pub fn record_started(&self, machine_id: &str, agent_id: &str, runtime_pid: u32) {
        let mut pids = self.instance_pids.lock().unwrap();
        pids.entry(machine_id.to_string())
            .or_default()
            .insert(agent_id.to_string(), runtime_pid);
    }

    /// Check whether an `agent.state` payload's `runtime_pid` matches the
    /// current instance pid for this `(machine_id, agent_id)`. Returns
    /// `true` if the payload is current and should be acted on, `false`
    /// if it's a stale frame from a previous instance and should be
    /// dropped. If we have no record for this agent (most commonly:
    /// first transition we've ever seen, or after a deregister), the
    /// frame is accepted by default — we'd rather act on a state we
    /// haven't tracked yet than drop a real transition.
    pub fn is_current_pid(&self, machine_id: &str, agent_id: &str, runtime_pid: u32) -> bool {
        let pids = self.instance_pids.lock().unwrap();
        match pids.get(machine_id).and_then(|m| m.get(agent_id)) {
            Some(&current) if current != runtime_pid => {
                self.stale_state_drops.fetch_add(1, Ordering::Relaxed);
                false
            }
            _ => true,
        }
    }

    /// Telemetry: how many `agent.state` frames the registry's filter
    /// has dropped because their `runtime_pid` was stale. Useful for
    /// tests; intended to be exposed on a future `/metrics` endpoint.
    pub fn stale_state_drops(&self) -> usize {
        self.stale_state_drops.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn connection_count(&self) -> usize {
        self.connections
            .lock()
            .unwrap()
            .values()
            .map(|v| v.len())
            .sum()
    }
}

/// RAII guard that removes the bridge's registration on drop. Held by
/// the WS session task for the lifetime of the connection.
pub struct Registration {
    registry: Arc<BridgeRegistry>,
    machine_id: String,
    sender: mpsc::Sender<String>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.registry.deregister(&self.machine_id, &self.sender);
        debug!(machine_id = %self.machine_id, "bridge registration dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_broadcast_delivers_to_connected_bridge() {
        let reg = BridgeRegistry::new();
        let (mut rx, _guard) = reg.register("m-1");
        assert_eq!(reg.connection_count(), 1);

        let delivered = reg.broadcast(r#"{"hello":"world"}"#);
        assert_eq!(delivered, 1);

        let received = rx.recv().await.unwrap();
        assert_eq!(received, r#"{"hello":"world"}"#);
    }

    #[tokio::test]
    async fn drop_guard_deregisters() {
        let reg = BridgeRegistry::new();
        let (_rx, guard) = reg.register("m-1");
        assert_eq!(reg.connection_count(), 1);
        drop(guard);
        assert_eq!(reg.connection_count(), 0);
    }

    #[tokio::test]
    async fn broadcast_to_multiple_machines() {
        let reg = BridgeRegistry::new();
        let (mut rx_a, _ga) = reg.register("m-a");
        let (mut rx_b, _gb) = reg.register("m-b");

        assert_eq!(reg.broadcast(r#"{"frame":"x"}"#), 2);
        assert_eq!(rx_a.recv().await.unwrap(), r#"{"frame":"x"}"#);
        assert_eq!(rx_b.recv().await.unwrap(), r#"{"frame":"x"}"#);
    }

    // ── slice 3: runtime_pid filtering ──

    #[tokio::test]
    async fn record_started_then_is_current_pid_matches() {
        let reg = BridgeRegistry::new();
        reg.record_started("m-1", "agent-a", 100);

        assert!(reg.is_current_pid("m-1", "agent-a", 100));
        assert_eq!(reg.stale_state_drops(), 0);
    }

    #[tokio::test]
    async fn stale_pid_is_dropped_and_counted() {
        let reg = BridgeRegistry::new();
        reg.record_started("m-1", "agent-a", 100);
        // Now imagine: bridge restarts the runtime → new pid 200.
        reg.record_started("m-1", "agent-a", 200);

        // A delayed `crashed` from the old pid arrives — must be dropped.
        assert!(!reg.is_current_pid("m-1", "agent-a", 100));
        assert_eq!(reg.stale_state_drops(), 1);

        // The new pid still passes.
        assert!(reg.is_current_pid("m-1", "agent-a", 200));
        assert_eq!(reg.stale_state_drops(), 1);
    }

    #[tokio::test]
    async fn unknown_pid_is_accepted_by_default() {
        // An agent we've never seen a `started` for — we accept the
        // first state we hear about. This avoids dropping legitimate
        // events when the platform restarts mid-session and rebuilds
        // its tracker from scratch.
        let reg = BridgeRegistry::new();
        assert!(reg.is_current_pid("m-1", "ghost-agent", 999));
        assert_eq!(reg.stale_state_drops(), 0);
    }

    #[tokio::test]
    async fn pids_cleared_on_last_disconnect() {
        let reg = BridgeRegistry::new();
        let (_rx, guard) = reg.register("m-1");
        reg.record_started("m-1", "agent-a", 100);
        assert!(reg.is_current_pid("m-1", "agent-a", 100));

        drop(guard); // last connection for m-1 closes

        // After cleanup, the tracker has no entry for m-1, so any pid
        // is accepted by default (see `unknown_pid_is_accepted_by_default`).
        assert!(reg.is_current_pid("m-1", "agent-a", 999));
        assert_eq!(reg.stale_state_drops(), 0);
    }
}
