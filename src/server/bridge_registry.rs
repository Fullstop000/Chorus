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
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tracing::debug;

/// Cap on outbound frames buffered per bridge before drop. Lifecycle and
/// chat frames are small (<1 KB); 256 leaves headroom for normal bursts.
const PER_BRIDGE_BUFFER: usize = 256;

/// Maps `machine_id` to a list of outbound senders (one per active WS
/// session for that machine). Each sender carries pre-encoded JSON
/// strings ready to write to a `Message::Text` frame.
#[derive(Default)]
pub struct BridgeRegistry {
    connections: Mutex<HashMap<String, Vec<mpsc::Sender<String>>>>,
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
        let mut conns = self.connections.lock().unwrap();
        if let Some(list) = conns.get_mut(machine_id) {
            list.retain(|tx| !tx.same_channel(sender));
            if list.is_empty() {
                conns.remove(machine_id);
            }
        }
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
}
