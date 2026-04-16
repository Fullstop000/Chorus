//! Pairing token store for the shared MCP bridge.
//!
//! Tokens are one-time credentials that bind an MCP session to an `agent_key`.
//! Generated via `chorus bridge-pair --agent <key>`, consumed on the first MCP
//! request to `/token/<token>/mcp`. A 5-minute TTL keeps an unused token from
//! lingering long enough to be leaked through shell history or config files.
//!
//! See `docs/BRIDGE_MIGRATION.md` for how this fits into the Phase 2 migration.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use tokio::sync::RwLock;

/// Default token lifetime (5 minutes).
///
/// Long enough for a human to paste the token into a config file and start
/// an agent; short enough that a leaked token is unlikely to be usable by
/// the time an attacker sees it.
const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// In-memory token store.
///
/// Tokens are keyed by opaque random strings (256 bits of entropy, URL-safe
/// base64). Only the bridge process holds the map — on restart all pending
/// tokens vanish, which is intentional: a new bridge is a new trust boundary.
pub struct PairingTokenStore {
    inner: Arc<RwLock<HashMap<String, TokenEntry>>>,
    ttl: Duration,
}

struct TokenEntry {
    agent_key: String,
    expires_at: Instant,
}

impl PairingTokenStore {
    /// Create a store with the default 5-minute TTL.
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    /// Create a store with a custom TTL (primarily for tests).
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Issue a fresh pairing token bound to `agent_key`.
    ///
    /// The returned string is a URL-safe base64 encoding of 32 random bytes.
    pub async fn issue(&self, agent_key: String) -> String {
        let token = generate_token();
        let mut map = self.inner.write().await;
        map.insert(
            token.clone(),
            TokenEntry {
                agent_key,
                expires_at: Instant::now() + self.ttl,
            },
        );
        token
    }

    /// Consume a token, returning the bound `agent_key` iff it exists and is
    /// not expired. The token is removed either way — one-time use only.
    pub async fn consume(&self, token: &str) -> Option<String> {
        let mut map = self.inner.write().await;
        // Remove up-front so an expired token is cleaned up on the first
        // attempted use, not just during a sweep.
        let entry = map.remove(token)?;
        if Instant::now() < entry.expires_at {
            Some(entry.agent_key)
        } else {
            None
        }
    }

    /// Drop every entry whose TTL has elapsed. Returns the number removed.
    pub async fn evict_expired(&self) -> usize {
        let mut map = self.inner.write().await;
        let now = Instant::now();
        let before = map.len();
        map.retain(|_, entry| now < entry.expires_at);
        before - map.len()
    }

    /// Number of tokens currently tracked (expired or not).
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }
}

impl Default for PairingTokenStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate 32 bytes of cryptographic randomness and encode as URL-safe
/// base64 (no padding). 32 bytes = 256 bits of entropy.
fn generate_token() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

/// Implementation of `chorus bridge-pair --agent <key>`.
///
/// Discovers the running bridge via `~/.chorus/bridge.json`, POSTs to its
/// `/admin/pair` endpoint, and prints the returned token plus a ready-to-use
/// connect URL.
pub async fn run_bridge_pair(agent_key: &str) -> anyhow::Result<()> {
    let info = crate::bridge::discovery::read_bridge_info().ok_or_else(|| {
        anyhow::anyhow!(
            "no running bridge found — start one with `chorus bridge-serve` first"
        )
    })?;

    let url = format!("http://127.0.0.1:{}/admin/pair", info.port);
    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .json(&serde_json::json!({ "agent_key": agent_key }))
        .send()
        .await
        .with_context(|| format!("failed to reach bridge at {url}"))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        anyhow::bail!("bridge returned {status}: {body}");
    }

    let response: serde_json::Value = res
        .json()
        .await
        .context("bridge returned an invalid JSON response")?;
    let token = response["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("bridge response missing 'token' field: {response}"))?;

    println!("Pairing token issued for agent '{agent_key}':");
    println!();
    println!("  Token: {token}");
    println!(
        "  URL:   http://127.0.0.1:{}/token/{}/mcp",
        info.port, token
    );
    println!();
    println!("Token expires in 5 minutes. Use it in your agent's MCP config.");

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn issue_and_consume() {
        let store = PairingTokenStore::new();
        let token = store.issue("agent-a".to_string()).await;
        let agent = store.consume(&token).await;
        assert_eq!(agent, Some("agent-a".to_string()));
    }

    #[tokio::test]
    async fn consume_twice_fails() {
        let store = PairingTokenStore::new();
        let token = store.issue("agent-a".to_string()).await;
        assert_eq!(store.consume(&token).await, Some("agent-a".to_string()));
        // Second consume must return None — tokens are one-time.
        assert_eq!(store.consume(&token).await, None);
    }

    #[tokio::test]
    async fn consume_invalid_fails() {
        let store = PairingTokenStore::new();
        assert_eq!(store.consume("not-a-real-token").await, None);
    }

    #[tokio::test]
    async fn expired_token_rejected() {
        let store = PairingTokenStore::with_ttl(Duration::from_millis(50));
        let token = store.issue("agent-a".to_string()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(store.consume(&token).await, None);
    }

    #[tokio::test]
    async fn evict_expired_removes_stale() {
        let store = PairingTokenStore::with_ttl(Duration::from_millis(50));
        let _t1 = store.issue("agent-a".to_string()).await;
        let _t2 = store.issue("agent-b".to_string()).await;
        assert_eq!(store.len().await, 2);

        tokio::time::sleep(Duration::from_millis(100)).await;
        let evicted = store.evict_expired().await;
        assert_eq!(evicted, 2);
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn different_tokens_different_agents() {
        let store = PairingTokenStore::new();
        let t_a = store.issue("agent-a".to_string()).await;
        let t_b = store.issue("agent-b".to_string()).await;

        assert_ne!(t_a, t_b, "tokens must be unique");
        assert_eq!(store.consume(&t_a).await, Some("agent-a".to_string()));
        assert_eq!(store.consume(&t_b).await, Some("agent-b".to_string()));
    }

    #[test]
    fn generate_token_is_unique_and_urlsafe() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b, "two tokens should almost never collide");
        // URL-safe base64 without padding uses only these characters.
        for ch in a.chars() {
            assert!(
                ch.is_ascii_alphanumeric() || ch == '-' || ch == '_',
                "token contains non-URL-safe char: {ch}"
            );
        }
    }
}
