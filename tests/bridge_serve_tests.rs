mod harness;
use std::sync::Arc;

use chorus::agent::runtime_status::{SharedRuntimeStatusProvider, SystemRuntimeStatusProvider};
use chorus::agent::AgentLifecycle;
use chorus::bridge::serve::build_bridge_router;
use chorus::server::build_router_with_services;
use chorus::store::channels::ChannelType;
use chorus::store::messages::ReceivedMessage;
use chorus::store::Store;
use harness::join_channel_silent;

/// Insert an agent row with a chosen primary key. Mirrors the helper used in
/// `server_tests`/`e2e_tests` so identity-typed args (member_id, sender_id)
/// can use the agent's name as its id, keeping URL paths like
/// `/internal/agent/bot1/...` and membership checks consistent under the
/// strict ID-first store.
#[allow(dead_code)]
fn seed_agent_with_id(
    store: &Arc<Store>,
    id: &str,
    display_name: &str,
    runtime: &str,
    model: &str,
) {
    let workspace_id = store
        .get_active_workspace()
        .unwrap()
        .expect("seed_agent_with_id requires an active workspace")
        .id;
    let conn = store.conn_for_test();
    conn.execute(
        "INSERT INTO agents (id, workspace_id, name, display_name, runtime, model)
         VALUES (?1, ?2, ?1, ?3, ?4, ?5)",
        rusqlite::params![id, workspace_id, display_name, runtime, model],
    )
    .unwrap();
}

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

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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

/// No-op lifecycle used when running the Chorus server in-process for tests.
/// Mirrors the helper in `tests/harness/mod.rs` — duplicated here because
/// integration tests cannot share test-only modules without extra wiring.
struct NoopLifecycle;

impl AgentLifecycle for NoopLifecycle {
    fn start_agent<'a>(
        &'a self,
        _agent_name: &'a str,
        _wake_message: Option<ReceivedMessage>,
        _init_directive: Option<String>,
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

    fn process_state<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Option<chorus::agent::drivers::ProcessState>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async { None })
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
    // Pre-create `#all` so the bootstrap migration in `ensure_builtin_channels`
    // does not rename `#general` to `#all` when the router is built below.
    store
        .create_channel(
            Store::DEFAULT_SYSTEM_CHANNEL,
            None,
            ChannelType::System,
            None,
        )
        .unwrap();
    store.ensure_human_with_id("testuser", "testuser").unwrap();
    store
        .create_channel("general", Some("General"), ChannelType::Channel, None)
        .unwrap();
    join_channel_silent(&store, "general", "testuser", "human");

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

/// Send an MCP initialize POST to the given URL with the given agent key
/// in the `X-Agent-Id` header, and return the response.
async fn send_initialize(
    client: &reqwest::Client,
    url: &str,
    agent_key: &str,
) -> reqwest::Response {
    client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("X-Agent-Id", agent_key)
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

    let resp_a = send_initialize(&client, &format!("{}/mcp", addr), "agent-a").await;
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
    assert_eq!(
        json_a["jsonrpc"], "2.0",
        "agent-a should return valid JSON-RPC"
    );

    let resp_b = send_initialize(&client, &format!("{}/mcp", addr), "agent-b").await;
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
    assert_eq!(
        json_b["jsonrpc"], "2.0",
        "agent-b should return valid JSON-RPC"
    );

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
    let url = format!("{}/mcp", addr);

    let resp1 = send_initialize(&client, &url, "agent-a").await;
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

    let resp2 = send_initialize(&client, &url, "agent-a").await;
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
    seed_agent_with_id(&store, "bot1", "Bot 1", "claude", "sonnet");
    join_channel_silent(&store, "general", "bot1", "agent");

    // 2. Start the bridge pointed at the Chorus server.
    let (bridge_addr, bridge_ct) = start_bridge_with_server(&server_url).await;
    let client = reqwest::Client::new();
    let mcp_url = format!("{}/mcp", bridge_addr);

    // 3. MCP initialize — grab the session ID out of the response headers.
    let init_resp = send_initialize(&client, &mcp_url, "bot1").await;
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
        .header("X-Agent-Id", "bot1")
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
        .header("X-Agent-Id", "bot1")
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
    let (messages, _has_more) = store.get_history("general", 100, None, None).unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.content.contains("Hello from bridge test!") && m.sender_name == "bot1"),
        "expected bridge-sent message in store, got: {:?}",
        messages
            .iter()
            .map(|m| (&m.sender_name, &m.content))
            .collect::<Vec<_>>()
    );

    bridge_ct.cancel();
}

// ---------------------------------------------------------------------------
// Agent-readability tests: task events reach agents as plain English content.
// The producer writes the human sentence into `messages.content` and the
// structured shape into `messages.payload`. The bridge no longer reformats —
// it returns `content` verbatim — so these tests assert the produced sentence
// flows through both read_history and check_messages.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bridge_read_history_surfaces_task_event_content() {
    use chorus::bridge::backend::{Backend, ChorusBackend};
    use chorus::store::channels::ChannelType;
    use chorus::store::messages::SenderType;

    let (server_url, store) = start_chorus_server().await;

    store
        .create_channel("eng", None, ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "eng", "alice", "human");
    seed_agent_with_id(&store, "agent-one", "agent-one", "claude", "sonnet");
    join_channel_silent(&store, "eng", "agent-one", "agent");

    store
        .create_tasks("eng", "alice", SenderType::Human, &["wire up"])
        .unwrap();

    let backend = ChorusBackend::new(server_url);
    let history = backend
        .read_history("agent-one", "eng", Some(20), None, None)
        .await
        .expect("read_history should succeed");

    assert!(
        history.contains(r#"alice created #1 "wire up""#),
        "expected human-readable task-event sentence in history, got:\n{}",
        history
    );
    assert!(
        !history.contains(r#""kind":"task_event""#),
        "raw payload JSON must NOT appear in agent-facing history; got:\n{}",
        history
    );
}

#[tokio::test]
async fn bridge_check_messages_surfaces_task_event_content() {
    use chorus::bridge::backend::{Backend, ChorusBackend};
    use chorus::store::channels::ChannelType;
    use chorus::store::messages::SenderType;
    use std::time::Duration;
    use tokio::time::timeout;

    let (server_url, store) = start_chorus_server().await;

    store
        .create_channel("eng", None, ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "eng", "alice", "human");
    seed_agent_with_id(&store, "agent-one", "agent-one", "claude", "sonnet");
    join_channel_silent(&store, "eng", "agent-one", "agent");

    store
        .create_tasks("eng", "alice", SenderType::Human, &["wire up"])
        .unwrap();

    let backend = ChorusBackend::new(server_url);
    let expected = r#"alice created #1 "wire up""#;

    let received = timeout(Duration::from_secs(2), async {
        loop {
            let messages = backend
                .check_messages("agent-one")
                .await
                .expect("check_messages should succeed");
            if messages.contains(expected) {
                break messages;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("timed out waiting for task-event sentence via check_messages");

    assert!(
        received.contains(expected),
        "expected human-readable task-event sentence from check_messages, got:\n{}",
        received
    );
    assert!(
        !received.contains(r#""kind":"task_event""#),
        "raw payload JSON must NOT appear in check_messages output; got:\n{}",
        received
    );
}
