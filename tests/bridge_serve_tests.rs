use std::sync::Arc;
use std::time::Duration;

use chorus::agent::runtime_status::{SharedRuntimeStatusProvider, SystemRuntimeStatusProvider};
use chorus::agent::AgentLifecycle;
use chorus::bridge::serve::{build_bridge_router, build_bridge_router_with_token_ttl};
use chorus::server::build_router_with_services;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{ReceivedMessage, SenderType};
use chorus::store::{AgentRecordUpsert, Store};

/// Helper: start the bridge server on a random port and return the base URL
/// and a cancellation token for graceful shutdown.
async fn start_bridge() -> (String, tokio_util::sync::CancellationToken) {
    start_bridge_with_server("http://localhost:1").await
}

/// Helper: start the bridge server on a random port, pointing at the provided
/// Chorus server URL. Returns the bridge base URL and its cancellation token.
async fn start_bridge_with_server(
    server_url: &str,
) -> (String, tokio_util::sync::CancellationToken) {
    let (app, ct) = build_bridge_router(server_url);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr = format!("http://127.0.0.1:{}", port);

    let shutdown_ct = ct.clone();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown_ct.cancelled().await })
            .await
            .unwrap();
    });

    // Give the server a moment to be ready.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (addr, ct)
}

/// Helper: start the bridge with a custom pairing-token TTL (for expiry tests).
async fn start_bridge_with_token_ttl(
    token_ttl: Duration,
) -> (String, tokio_util::sync::CancellationToken) {
    let (app, ct) = build_bridge_router_with_token_ttl("http://localhost:1", Some(token_ttl));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr = format!("http://127.0.0.1:{}", port);

    let shutdown_ct = ct.clone();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown_ct.cancelled().await })
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    (addr, ct)
}

/// Helper: POST `/admin/pair` and return the issued token.
async fn issue_pairing_token(
    client: &reqwest::Client,
    bridge_addr: &str,
    agent_key: &str,
) -> String {
    let resp = client
        .post(format!("{}/admin/pair", bridge_addr))
        .json(&serde_json::json!({ "agent_key": agent_key }))
        .send()
        .await
        .expect("pair request should succeed");
    assert_eq!(resp.status(), 200, "pair should return 200");
    let body: serde_json::Value = resp.json().await.expect("pair body should be JSON");
    body["token"]
        .as_str()
        .expect("pair response should contain 'token'")
        .to_string()
}

/// No-op lifecycle used when running the Chorus server in-process for tests.
/// Mirrors the helper in `tests/harness/mod.rs` — duplicated here because
/// integration tests cannot share test-only modules without extra wiring.
struct NoopLifecycle;

impl AgentLifecycle for NoopLifecycle {
    fn start_agent<'a>(
        &'a self,
        _agent_name: &'a str,
        _wake_message: Option<ReceivedMessage>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn notify_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn stop_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn get_activity_log_data(
        &self,
        _agent_name: &str,
        _after_seq: Option<u64>,
    ) -> chorus::agent::activity_log::ActivityLogResponse {
        chorus::agent::activity_log::ActivityLogResponse {
            entries: vec![],
            agent_activity: "offline".to_string(),
            agent_detail: String::new(),
        }
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        vec![]
    }
}

/// Start a Chorus server in-process with an in-memory SQLite store. Returns
/// the server's base URL, the shared `Store`, and a join handle. The server
/// is spawned on a background task and lives for the duration of the test.
async fn start_chorus_server() -> (String, Arc<Store>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.create_human("testuser").unwrap();
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store
        .join_channel("general", "testuser", SenderType::Human)
        .unwrap();

    let router = build_router_with_services(
        store.clone(),
        Arc::new(NoopLifecycle),
        Arc::new(SystemRuntimeStatusProvider::new(
            chorus::agent::manager::build_driver_registry(),
        )) as SharedRuntimeStatusProvider,
        vec![],
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (url, store)
}

/// Build a JSON-RPC initialize request body.
fn initialize_body() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "test-agent", "version": "1.0"}
        }
    })
}

/// Send an MCP initialize POST to the given URL and return the response.
async fn send_initialize(client: &reqwest::Client, url: &str) -> reqwest::Response {
    client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&initialize_body())
        .send()
        .await
        .expect("request should succeed")
}

/// Parse the SSE response body and extract the first JSON-RPC data line.
fn extract_jsonrpc_from_sse(body: &str) -> serde_json::Value {
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                return val;
            }
        }
    }
    panic!(
        "no valid JSON-RPC data line found in SSE response body:\n{}",
        body
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bridge_starts_and_health_check() {
    let (addr, ct) = start_bridge().await;

    let resp = reqwest::get(format!("{}/health", addr))
        .await
        .expect("health request should succeed");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");

    ct.cancel();
}

#[tokio::test]
async fn two_agents_get_separate_sessions() {
    let (addr, ct) = start_bridge().await;
    let client = reqwest::Client::new();

    let resp_a = send_initialize(&client, &format!("{}/agent-a/mcp", addr)).await;
    assert_eq!(resp_a.status(), 200, "agent-a initialize should return 200");
    let session_a = resp_a
        .headers()
        .get("Mcp-Session-Id")
        .expect("agent-a response should contain Mcp-Session-Id header")
        .to_str()
        .unwrap()
        .to_owned();
    let body_a = resp_a.text().await.unwrap();
    let json_a = extract_jsonrpc_from_sse(&body_a);
    assert_eq!(json_a["jsonrpc"], "2.0", "agent-a should return valid JSON-RPC");

    let resp_b = send_initialize(&client, &format!("{}/agent-b/mcp", addr)).await;
    assert_eq!(resp_b.status(), 200, "agent-b initialize should return 200");
    let session_b = resp_b
        .headers()
        .get("Mcp-Session-Id")
        .expect("agent-b response should contain Mcp-Session-Id header")
        .to_str()
        .unwrap()
        .to_owned();
    let body_b = resp_b.text().await.unwrap();
    let json_b = extract_jsonrpc_from_sse(&body_b);
    assert_eq!(json_b["jsonrpc"], "2.0", "agent-b should return valid JSON-RPC");

    assert_ne!(
        session_a, session_b,
        "agent-a and agent-b must get different session IDs"
    );

    ct.cancel();
}

#[tokio::test]
async fn same_agent_reuses_service() {
    let (addr, ct) = start_bridge().await;
    let client = reqwest::Client::new();
    let url = format!("{}/agent-a/mcp", addr);

    let resp1 = send_initialize(&client, &url).await;
    assert_eq!(resp1.status(), 200, "first initialize should return 200");
    let session1 = resp1
        .headers()
        .get("Mcp-Session-Id")
        .expect("first response should contain Mcp-Session-Id")
        .to_str()
        .unwrap()
        .to_owned();
    // Consume the body so the connection is released.
    let body1 = resp1.text().await.unwrap();
    let json1 = extract_jsonrpc_from_sse(&body1);
    assert_eq!(json1["jsonrpc"], "2.0");

    let resp2 = send_initialize(&client, &url).await;
    assert_eq!(resp2.status(), 200, "second initialize should return 200");
    let session2 = resp2
        .headers()
        .get("Mcp-Session-Id")
        .expect("second response should contain Mcp-Session-Id")
        .to_str()
        .unwrap()
        .to_owned();
    let body2 = resp2.text().await.unwrap();
    let json2 = extract_jsonrpc_from_sse(&body2);
    assert_eq!(json2["jsonrpc"], "2.0");

    // Both calls hit agent-a's service — they should both succeed.
    // The sessions may be different (each initialize creates a new MCP session),
    // but that's fine — the key thing is that both requests succeed, proving
    // the per-agent service handles multiple session creations.
    assert!(
        !session1.is_empty() && !session2.is_empty(),
        "both sessions should be non-empty"
    );

    ct.cancel();
}

/// Full end-to-end: MCP client -> bridge HTTP -> ChatBridge -> ChorusBackend
/// -> Chorus server -> SQLite store. Proves that a `send_message` tool call
/// dispatched through the bridge actually lands in the Chorus store.
#[tokio::test]
async fn bridge_sends_message_to_chorus_server() {
    // 1. Start the Chorus server with a seeded channel + agent.
    let (server_url, store) = start_chorus_server().await;
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    // 2. Start the bridge pointed at the Chorus server.
    let (bridge_addr, bridge_ct) = start_bridge_with_server(&server_url).await;
    let client = reqwest::Client::new();
    let mcp_url = format!("{}/bot1/mcp", bridge_addr);

    // 3. MCP initialize — grab the session ID out of the response headers.
    let init_resp = send_initialize(&client, &mcp_url).await;
    assert_eq!(init_resp.status(), 200, "initialize should return 200");
    let session_id = init_resp
        .headers()
        .get("Mcp-Session-Id")
        .expect("initialize response must contain Mcp-Session-Id")
        .to_str()
        .unwrap()
        .to_owned();
    // Drain the init body so the connection is released.
    let _ = init_resp.text().await.unwrap();

    // 4. Send the required `notifications/initialized` to complete the MCP
    //    handshake before issuing any tool calls. This is a JSON-RPC
    //    notification (no `id`) and expects 202 Accepted.
    let initialized_resp = client
        .post(&mcp_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .await
        .expect("initialized notification should succeed");
    assert!(
        initialized_resp.status().is_success(),
        "initialized notification should succeed, got {}",
        initialized_resp.status()
    );
    let _ = initialized_resp.text().await.unwrap();

    // 5. Call `send_message` via tools/call, using the session ID from init.
    let tools_call_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "send_message",
            "arguments": {
                "target": "#general",
                "content": "Hello from bridge test!"
            }
        }
    });
    let call_resp = client
        .post(&mcp_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .json(&tools_call_body)
        .send()
        .await
        .expect("tools/call request should succeed");
    assert_eq!(call_resp.status(), 200, "tools/call should return 200");
    let call_body = call_resp.text().await.unwrap();
    let call_json = extract_jsonrpc_from_sse(&call_body);
    assert_eq!(call_json["jsonrpc"], "2.0");
    assert_eq!(call_json["id"], 2);
    assert!(
        call_json.get("error").is_none(),
        "tools/call should not return an error, got: {}",
        call_json
    );
    assert!(
        call_json["result"].is_object(),
        "tools/call should return a result object, got: {}",
        call_json
    );

    // 6. Verify the message actually landed in the Chorus store.
    let (messages, _has_more) = store.get_history("general", None, 100, None, None).unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.content.contains("Hello from bridge test!")
                && m.sender_name == "bot1"),
        "expected bridge-sent message in store, got: {:?}",
        messages
            .iter()
            .map(|m| (&m.sender_name, &m.content))
            .collect::<Vec<_>>()
    );

    bridge_ct.cancel();
}

// ---------------------------------------------------------------------------
// Pairing token tests (Phase 2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bridge_pair_issues_token() {
    let (addr, ct) = start_bridge().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/admin/pair", addr))
        .json(&serde_json::json!({ "agent_key": "bot1" }))
        .send()
        .await
        .expect("pair should succeed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().expect("response should include token");
    assert!(!token.is_empty(), "token should be non-empty");
    // URL-safe base64 uses only these chars.
    assert!(
        token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "token should be URL-safe: {}",
        token
    );

    ct.cancel();
}

#[tokio::test]
async fn bridge_pair_rejects_invalid_agent_key() {
    let (addr, ct) = start_bridge().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/admin/pair", addr))
        .json(&serde_json::json!({ "agent_key": "../etc/passwd" }))
        .send()
        .await
        .expect("pair should respond");
    assert_eq!(resp.status(), 400);

    ct.cancel();
}

#[tokio::test]
async fn token_connects_to_agent_mcp() {
    let (addr, ct) = start_bridge().await;
    let client = reqwest::Client::new();

    // 1. Mint a token for bot1.
    let token = issue_pairing_token(&client, &addr, "bot1").await;

    // 2. Initialize against the token URL — should succeed.
    let token_url = format!("{}/token/{}/mcp", addr, token);
    let resp = send_initialize(&client, &token_url).await;
    assert_eq!(resp.status(), 200, "token-based initialize should return 200");

    let session_id = resp
        .headers()
        .get("Mcp-Session-Id")
        .expect("token init should return a session ID")
        .to_str()
        .unwrap()
        .to_owned();
    assert!(!session_id.is_empty(), "session ID must be non-empty");

    // 3. Second request on the same URL must still work (token-to-agent cache
    //    keeps the mapping alive for the session).
    let body = resp.text().await.unwrap();
    let json = extract_jsonrpc_from_sse(&body);
    assert_eq!(json["jsonrpc"], "2.0");

    let follow_up = client
        .post(&token_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .await
        .expect("follow-up should succeed");
    assert!(
        follow_up.status().is_success(),
        "second request on same token URL should succeed, got {}",
        follow_up.status()
    );

    ct.cancel();
}

#[tokio::test]
async fn invalid_token_returns_unauthorized() {
    let (addr, ct) = start_bridge().await;
    let client = reqwest::Client::new();

    let resp = send_initialize(
        &client,
        &format!("{}/token/not-a-real-token/mcp", addr),
    )
    .await;
    assert_eq!(
        resp.status(),
        401,
        "unknown token must 401, got {}",
        resp.status()
    );

    ct.cancel();
}

#[tokio::test]
async fn expired_token_rejected() {
    // Use a very short TTL so the test doesn't have to wait 5 minutes.
    let (addr, ct) = start_bridge_with_token_ttl(Duration::from_millis(100)).await;
    let client = reqwest::Client::new();

    let token = issue_pairing_token(&client, &addr, "bot1").await;

    // Wait past the TTL.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let resp = send_initialize(&client, &format!("{}/token/{}/mcp", addr, token)).await;
    assert_eq!(
        resp.status(),
        401,
        "expired token must 401, got {}",
        resp.status()
    );

    ct.cancel();
}

#[tokio::test]
async fn pairing_token_end_to_end_sends_message() {
    // 1. Start Chorus server with bot1 joined to #general.
    let (server_url, store) = start_chorus_server().await;
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    // 2. Start bridge pointed at Chorus.
    let (bridge_addr, bridge_ct) = start_bridge_with_server(&server_url).await;
    let client = reqwest::Client::new();

    // 3. Mint token and use it for full MCP handshake.
    let token = issue_pairing_token(&client, &bridge_addr, "bot1").await;
    let mcp_url = format!("{}/token/{}/mcp", bridge_addr, token);

    let init_resp = send_initialize(&client, &mcp_url).await;
    assert_eq!(init_resp.status(), 200);
    let session_id = init_resp
        .headers()
        .get("Mcp-Session-Id")
        .expect("init should return session ID")
        .to_str()
        .unwrap()
        .to_owned();
    let _ = init_resp.text().await.unwrap();

    let initialized_resp = client
        .post(&mcp_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .await
        .expect("initialized should succeed");
    assert!(initialized_resp.status().is_success());
    let _ = initialized_resp.text().await.unwrap();

    // 4. send_message via tools/call using the token URL.
    let call_resp = client
        .post(&mcp_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "send_message",
                "arguments": {
                    "target": "#general",
                    "content": "Hello from token-paired bridge!"
                }
            }
        }))
        .send()
        .await
        .expect("tools/call should succeed");
    assert_eq!(call_resp.status(), 200);
    let call_body = call_resp.text().await.unwrap();
    let call_json = extract_jsonrpc_from_sse(&call_body);
    assert!(
        call_json.get("error").is_none(),
        "tools/call should not error, got: {}",
        call_json
    );

    // 5. Verify it landed in the store.
    let (messages, _) = store.get_history("general", None, 100, None, None).unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.content.contains("Hello from token-paired bridge!")
                && m.sender_name == "bot1"),
        "expected token-routed message in store"
    );

    bridge_ct.cancel();
}
