use tokio::sync::broadcast;

use crate::agent::trace::TraceEvent;
use crate::store::stream::StreamEvent;

/// Application-level event bus for real-time stream and trace broadcasts.
///
/// Decoupled from the persistence layer so [`Store`] remains a pure DAO
/// and can be replaced with Mongo/Supabase backends without carrying
/// WebSocket broadcast state.
#[derive(Clone)]
pub struct EventBus {
    stream_tx: broadcast::Sender<StreamEvent>,
    trace_tx: broadcast::Sender<TraceEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (stream_tx, _) = broadcast::channel(256);
        let (trace_tx, _) = broadcast::channel(1024);
        Self {
            stream_tx,
            trace_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StreamEvent> {
        self.stream_tx.subscribe()
    }

    pub fn subscribe_traces(&self) -> broadcast::Receiver<TraceEvent> {
        self.trace_tx.subscribe()
    }

    pub fn trace_sender(&self) -> broadcast::Sender<TraceEvent> {
        self.trace_tx.clone()
    }

    pub fn stream_sender(&self) -> broadcast::Sender<StreamEvent> {
        self.stream_tx.clone()
    }

    /// Publish a single stream event. Best-effort: errors are dropped
    /// because the DB rows are the source of truth.
    pub fn publish_stream(&self, event: StreamEvent) {
        let _ = self.stream_tx.send(event);
    }


}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
