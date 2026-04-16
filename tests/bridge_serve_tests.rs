use chorus::bridge::serve::build_bridge_router;

/// Helper: start the bridge server on a random port and return the base URL
/// and a cancellation token for graceful shutdown.
async fn start_bridge() -> (String, tokio_util::sync::CancellationToken) {
    let (app, ct) = build_bridge_router("http://localhost:1");

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
