use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("session registry full ({max} entries, {expired} expired)")]
    Full { max: usize, expired: usize },
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

struct SessionEntry {
    agent_key: String,
    #[allow(dead_code)] // Retained for future session-age metrics and diagnostics
    created_at: Instant,
    last_active: Instant,
}

struct RegistryInner {
    sessions: HashMap<String, SessionEntry>,
}

// ---------------------------------------------------------------------------
// SessionRegistry
// ---------------------------------------------------------------------------

/// Maps MCP session IDs to agent keys with TTL-based eviction.
pub struct SessionRegistry {
    inner: Arc<RwLock<RegistryInner>>,
    ttl: Duration,
    max_entries: usize,
}

impl SessionRegistry {
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner {
                sessions: HashMap::new(),
            })),
            ttl,
            max_entries,
        }
    }

    /// Register a session ID → agent key mapping.
    ///
    /// If the registry is full, an eviction pass is run first. If it is still
    /// full after eviction the call returns [`RegistryError::Full`].
    pub async fn register(&self, session_id: String, agent_key: String) -> Result<(), RegistryError> {
        let mut inner = self.inner.write().await;

        // Fast path: there is still room.
        if inner.sessions.len() < self.max_entries {
            let now = Instant::now();
            inner.sessions.insert(
                session_id,
                SessionEntry {
                    agent_key,
                    created_at: now,
                    last_active: now,
                },
            );
            return Ok(());
        }

        // Try to make room by evicting expired entries.
        let now = Instant::now();
        let ttl = self.ttl;
        let expired_count = {
            let before = inner.sessions.len();
            inner
                .sessions
                .retain(|_, entry| now.duration_since(entry.last_active) < ttl);
            before - inner.sessions.len()
        };

        if inner.sessions.len() < self.max_entries {
            let now = Instant::now();
            inner.sessions.insert(
                session_id,
                SessionEntry {
                    agent_key,
                    created_at: now,
                    last_active: now,
                },
            );
            Ok(())
        } else {
            Err(RegistryError::Full {
                max: self.max_entries,
                expired: expired_count,
            })
        }
    }

    /// Remove a session and return its associated agent key, or `None` if it
    /// did not exist.
    pub async fn remove(&self, session_id: &str) -> Option<String> {
        let mut inner = self.inner.write().await;
        inner.sessions.remove(session_id).map(|e| e.agent_key)
    }

    /// Look up the agent key for a session, updating `last_active` on a hit.
    pub async fn lookup(&self, session_id: &str) -> Option<String> {
        // Optimistic read to avoid write lock on miss.
        {
            let inner = self.inner.read().await;
            if !inner.sessions.contains_key(session_id) {
                return None;
            }
        }

        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.sessions.get_mut(session_id) {
            entry.last_active = Instant::now();
            Some(entry.agent_key.clone())
        } else {
            None
        }
    }

    /// Update `last_active` for a session without returning anything.
    pub async fn touch(&self, session_id: &str) {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.sessions.get_mut(session_id) {
            entry.last_active = Instant::now();
        }
    }

    /// Evict all entries whose `last_active` is older than TTL.
    ///
    /// Returns the number of entries removed.
    pub async fn evict_expired(&self) -> usize {
        let mut inner = self.inner.write().await;
        let before = inner.sessions.len();
        let ttl = self.ttl;
        let now = Instant::now();
        inner
            .sessions
            .retain(|_, entry| now.duration_since(entry.last_active) < ttl);
        before - inner.sessions.len()
    }

    /// Number of sessions currently tracked.
    pub async fn len(&self) -> usize {
        self.inner.read().await.sessions.len()
    }

    /// Returns all session IDs associated with the given agent key.
    pub async fn agent_sessions(&self, agent_key: &str) -> Vec<String> {
        let inner = self.inner.read().await;
        inner
            .sessions
            .iter()
            .filter(|(_, entry)| entry.agent_key == agent_key)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn register_and_lookup() {
        let reg = SessionRegistry::new(Duration::from_secs(60), 100);
        reg.register("sess-1".to_string(), "agent-a".to_string())
            .await
            .expect("register should succeed");
        let key = reg.lookup("sess-1").await;
        assert_eq!(key, Some("agent-a".to_string()));
    }

    #[tokio::test]
    async fn remove_returns_agent_key() {
        let reg = SessionRegistry::new(Duration::from_secs(60), 100);
        reg.register("sess-2".to_string(), "agent-b".to_string())
            .await
            .unwrap();
        let removed = reg.remove("sess-2").await;
        assert_eq!(removed, Some("agent-b".to_string()));
        assert_eq!(reg.lookup("sess-2").await, None);
    }

    #[tokio::test]
    async fn lookup_missing_returns_none() {
        let reg = SessionRegistry::new(Duration::from_secs(60), 100);
        assert_eq!(reg.lookup("does-not-exist").await, None);
    }

    #[tokio::test]
    async fn ttl_eviction() {
        let ttl = Duration::from_millis(100);
        let reg = SessionRegistry::new(ttl, 100);
        reg.register("sess-3".to_string(), "agent-c".to_string())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let evicted = reg.evict_expired().await;
        assert_eq!(evicted, 1);
        assert_eq!(reg.len().await, 0);
    }

    #[tokio::test]
    async fn max_entries_enforced() {
        let reg = SessionRegistry::new(Duration::from_secs(60), 2);
        reg.register("s1".to_string(), "agent-a".to_string())
            .await
            .unwrap();
        reg.register("s2".to_string(), "agent-b".to_string())
            .await
            .unwrap();
        // Third insert should fail because no entries are expired.
        let err = reg
            .register("s3".to_string(), "agent-c".to_string())
            .await;
        assert!(err.is_err(), "expected Full error");
        match err.unwrap_err() {
            RegistryError::Full { max, .. } => assert_eq!(max, 2),
        }
    }

    #[tokio::test]
    async fn touch_extends_lifetime() {
        let ttl = Duration::from_millis(200);
        let reg = SessionRegistry::new(ttl, 100);
        reg.register("sess-4".to_string(), "agent-d".to_string())
            .await
            .unwrap();

        // Touch just before TTL expires.
        tokio::time::sleep(Duration::from_millis(150)).await;
        reg.touch("sess-4").await;

        // Wait another interval that would have expired the original entry, but
        // not the touched one (total ~250ms since touch, still within TTL).
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should NOT be evicted because touch reset last_active.
        let evicted = reg.evict_expired().await;
        assert_eq!(evicted, 0, "entry should still be alive after touch");
        assert_eq!(reg.len().await, 1);
    }

    #[tokio::test]
    async fn agent_sessions_returns_all() {
        let reg = SessionRegistry::new(Duration::from_secs(60), 100);
        reg.register("s-a1".to_string(), "agent-x".to_string())
            .await
            .unwrap();
        reg.register("s-a2".to_string(), "agent-x".to_string())
            .await
            .unwrap();
        reg.register("s-b1".to_string(), "agent-y".to_string())
            .await
            .unwrap();

        let mut sessions = reg.agent_sessions("agent-x").await;
        sessions.sort();
        assert_eq!(sessions, vec!["s-a1".to_string(), "s-a2".to_string()]);

        let other = reg.agent_sessions("agent-y").await;
        assert_eq!(other, vec!["s-b1".to_string()]);
    }
}
