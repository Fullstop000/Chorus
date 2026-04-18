//! Smoke test for the shared bridge — starts a bridge, connects a client, verifies handshake.

use anyhow::{Context, Result};
use std::time::Duration;

/// Run the bridge smoke test. Returns Ok if the bridge server starts, accepts
/// an MCP initialize request on /{agent_key}/mcp, and returns a valid session ID.
/// Does NOT require a running Chorus server — the server_url points at a fake URL.
pub async fn run_smoke_test() -> Result<()> {
    println!("Starting bridge smoke test...");

    // 1. Bind bridge to random port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind bridge port")?;
    let port = listener.local_addr()?.port();
    let bridge_url = format!("http://127.0.0.1:{}", port);
    println!("  ✓ Bridge bound on port {}", port);

    // 2. Start bridge server with fake backend URL (we're only testing the MCP layer)
    let (router, ct) = crate::bridge::serve::build_bridge_router("http://localhost:1");
    let shutdown = ct.clone();

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct.cancelled().await })
            .await
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 3. Test health endpoint
    let client = reqwest::Client::new();
    let health = client
        .get(format!("{}/health", bridge_url))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .context("health check failed")?;
    if !health.status().is_success() {
        anyhow::bail!("health check returned {}", health.status());
    }
    println!("  ✓ Health check passed");

    // 4. Send MCP initialize request
    let init_body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1.0"}}}"#;

    let response = client
        .post(format!("{}/test-agent/mcp", bridge_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .timeout(Duration::from_secs(5))
        .body(init_body)
        .send()
        .await
        .context("initialize request failed")?;

    if !response.status().is_success() {
        anyhow::bail!("initialize returned {}", response.status());
    }

    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .context("missing Mcp-Session-Id header")?
        .to_string();
    println!(
        "  ✓ Initialize succeeded (session {})",
        &session_id[..8.min(session_id.len())]
    );

    // 5. Shutdown cleanly
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_handle).await;
    println!("  ✓ Graceful shutdown");

    println!("\nSmoke test PASSED — bridge is working.");
    Ok(())
}
