//! E2E tests for the Phase 3 bridge ↔ platform WebSocket (slice 1).
//!
//! Verifies the wire shape end-to-end: a real Axum server is bound to a
//! local TCP port, a `tokio-tungstenite` client connects to
//! `/api/bridge/ws`, sends a `bridge.hello` frame, and asserts the
//! `bridge.target` reply lists the agent records currently in the DB.

mod harness;

use std::sync::Arc;

use anyhow::Context;
use chorus::store::channels::ChannelType;
use chorus::store::AgentRecordUpsert;
use chorus::store::Store;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

async fn start_test_server() -> (String, Arc<Store>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    // Pre-seed the system + general channels so router bootstrap doesn't
    // rename anything during build_router.
    store
        .create_channel(
            Store::DEFAULT_SYSTEM_CHANNEL,
            None,
            ChannelType::System,
            None,
        )
        .unwrap();
    store
        .create_channel("general", Some("General"), ChannelType::Channel, None)
        .unwrap();
    let router = harness::build_router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (url, store)
}

async fn read_json_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    let frame = timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("expected websocket frame within 2s")
        .context("expected websocket frame")
        .unwrap()
        .context("websocket frame should be ok")
        .unwrap();
    let Message::Text(text) = frame else {
        panic!("expected text websocket frame, got: {frame:?}");
    };
    serde_json::from_str(text.as_str()).unwrap()
}

#[tokio::test]
async fn bridge_ws_hello_returns_target_with_agent_records() {
    let (base_url, store) = start_test_server().await;

    // Seed two agents so we can verify `target_agents` is populated and
    // ordered consistently.
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "alpha-bot",
            display_name: "Alpha Bot",
            description: None,
            system_prompt: Some("you are alpha"),
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "beta-bot",
            display_name: "Beta Bot",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5",
            reasoning_effort: Some("medium"),
            env_vars: &[],
        })
        .unwrap();

    let (mut socket, _) = connect_async(format!("{base_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade should succeed");

    let hello = json!({
        "v": 1,
        "type": "bridge.hello",
        "data": {
            "machine_id": "test-machine-001",
            "bridge_version": "0.0.0-test",
            "supported_frames": ["bridge.hello", "bridge.target", "agent.state"],
            "agents_alive": []
        }
    });
    socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .expect("send hello");

    let frame = read_json_frame(&mut socket).await;

    assert_eq!(frame["v"], 1, "envelope version");
    assert_eq!(frame["type"], "bridge.target", "frame type");

    let targets = frame["data"]["target_agents"]
        .as_array()
        .expect("target_agents should be an array");
    assert_eq!(targets.len(), 2, "both seeded agents should appear in target");

    // get_agents() orders by name; alpha-bot before beta-bot.
    assert_eq!(targets[0]["runtime"], "claude");
    assert_eq!(targets[0]["model"], "sonnet");
    assert_eq!(targets[0]["system_prompt"], "you are alpha");
    assert!(targets[0]["agent_id"].is_string(), "agent_id present");

    assert_eq!(targets[1]["runtime"], "codex");
    assert_eq!(targets[1]["model"], "gpt-5");
    assert!(targets[1]["system_prompt"].is_null());
}

#[tokio::test]
async fn bridge_ws_empty_target_when_no_agents() {
    let (base_url, _store) = start_test_server().await;

    let (mut socket, _) = connect_async(format!("{base_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade should succeed");

    let hello = json!({
        "v": 1,
        "type": "bridge.hello",
        "data": {
            "machine_id": "empty-machine",
            "bridge_version": "0.0.0-test"
        }
    });
    socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .unwrap();

    let frame = read_json_frame(&mut socket).await;
    assert_eq!(frame["type"], "bridge.target");
    let targets = frame["data"]["target_agents"].as_array().unwrap();
    assert_eq!(targets.len(), 0);
}

#[tokio::test]
async fn bridge_ws_rejects_non_hello_first_frame() {
    let (base_url, _store) = start_test_server().await;

    let (mut socket, _) = connect_async(format!("{base_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade should succeed");

    // Send a frame that is NOT bridge.hello — server should drop the
    // connection without replying.
    let bogus = json!({
        "v": 1,
        "type": "agent.state",
        "data": { "agent_id": "x", "state": "running", "runtime_pid": 42 }
    });
    socket
        .send(Message::Text(bogus.to_string().into()))
        .await
        .unwrap();

    // Expect the stream to close (no target frame ever arrives). Slice 1
    // drops the socket without a clean handshake on protocol violation, so
    // we accept clean Close, transport-level error, and unclean reset as
    // equivalent outcomes — what we're really checking is "no
    // bridge.target was sent."
    let next = timeout(Duration::from_millis(500), socket.next()).await;
    match next {
        Ok(None) => {}
        Ok(Some(Ok(Message::Close(_)))) => {}
        Ok(Some(Err(_))) => {}
        Ok(Some(Ok(Message::Text(text)))) => {
            panic!("server should have rejected the bogus frame, but sent: {text}");
        }
        other => panic!("unexpected post-bogus-frame outcome: {other:?}"),
    }
}
