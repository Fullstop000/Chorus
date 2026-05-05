//! E2E tests for the Phase 3 bridge ↔ platform WebSocket (slices 1-2).
//!
//! Slice 1: a real Axum server is bound to a local TCP port, a
//! `tokio-tungstenite` client connects to `/api/bridge/ws`, sends a
//! `bridge.hello` frame, and asserts the `bridge.target` reply lists
//! the agent records currently in the DB.
//!
//! Slice 2: after the initial target, a fresh `bridge.target` is pushed
//! whenever an agent is mutated through the HTTP API. The bridge can
//! send `agent.state` frames upstream and the session keeps running.

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

async fn start_test_server() -> (String, String, Arc<Store>) {
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
    let ws_url = format!("ws://{addr}");
    let http_url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (ws_url, http_url, store)
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
    let (base_url, _http_url, store) = start_test_server().await;

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
    assert_eq!(
        targets.len(),
        2,
        "both seeded agents should appear in target"
    );

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
    let (base_url, _http_url, _store) = start_test_server().await;

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
    let (base_url, _http_url, _store) = start_test_server().await;

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

// ── Slice 2 ────────────────────────────────────────────────────────────

async fn send_hello(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    machine_id: &str,
) {
    let hello = json!({
        "v": 1,
        "type": "bridge.hello",
        "data": {
            "machine_id": machine_id,
            "bridge_version": "0.0.0-test",
            "supported_frames": ["bridge.hello", "bridge.target", "agent.state", "chat.ack"],
            "agents_alive": []
        }
    });
    socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn bridge_ws_pushes_target_when_agent_created_via_http() {
    let (ws_url, http_url, _store) = start_test_server().await;

    let (mut socket, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade should succeed");

    send_hello(&mut socket, "slice2-machine").await;

    // Initial target frame is empty (no agents seeded).
    let initial = read_json_frame(&mut socket).await;
    assert_eq!(initial["type"], "bridge.target");
    assert_eq!(
        initial["data"]["target_agents"].as_array().unwrap().len(),
        0
    );

    // Create an agent over HTTP — this should trigger a pushed
    // bridge.target onto the open WS.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{http_url}/api/agents"))
        .json(&json!({
            "name": "push-bot",
            "display_name": "Push Bot",
            "systemPrompt": "pushed",
            "runtime": "claude",
            "model": "sonnet"
        }))
        .send()
        .await
        .expect("POST /api/agents")
        .error_for_status()
        .expect("agent creation should succeed");
    let created: Value = resp.json().await.unwrap();
    let created_id = created["id"].as_str().unwrap().to_string();

    // Now the WS should receive the pushed target frame.
    let pushed = read_json_frame(&mut socket).await;
    assert_eq!(pushed["type"], "bridge.target");
    let targets = pushed["data"]["target_agents"].as_array().unwrap();
    assert_eq!(
        targets.len(),
        1,
        "pushed target should include the just-created agent"
    );
    assert_eq!(targets[0]["agent_id"], created_id);
    assert_eq!(targets[0]["runtime"], "claude");
    assert_eq!(targets[0]["model"], "sonnet");
    assert_eq!(targets[0]["system_prompt"], "pushed");
}

#[tokio::test]
async fn bridge_ws_accepts_agent_state_frame_without_disconnecting() {
    let (ws_url, http_url, _store) = start_test_server().await;

    let (mut socket, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade should succeed");

    send_hello(&mut socket, "agent-state-machine").await;
    let _initial = read_json_frame(&mut socket).await; // drain initial empty target

    // Bridge sends a well-formed agent.state upstream. Slice 2 logs and
    // returns OK; later slices will track and persist the transition.
    let frame = json!({
        "v": 1,
        "type": "agent.state",
        "data": {
            "agent_id": "some-uuid",
            "state": "started",
            "ts": "2026-05-05T12:00:00Z",
            "runtime_pid": 99999
        }
    });
    socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();

    // The session should still be alive — trigger another push via HTTP
    // and assert a fresh target frame arrives.
    let client = reqwest::Client::new();
    client
        .post(format!("{http_url}/api/agents"))
        .json(&json!({
            "name": "after-state-bot",
            "display_name": "After State Bot",
            "runtime": "claude",
            "model": "sonnet"
        }))
        .send()
        .await
        .expect("POST /api/agents")
        .error_for_status()
        .expect("agent creation should succeed");

    let pushed = read_json_frame(&mut socket).await;
    assert_eq!(pushed["type"], "bridge.target");
    let targets = pushed["data"]["target_agents"].as_array().unwrap();
    assert_eq!(targets.len(), 1, "agent.state did not break the session");
}

#[tokio::test]
async fn bridge_ws_handles_stop_start_race_without_breaking_session() {
    // Slice 3: agent.state frames carry runtime_pid as the instance
    // discriminator. A delayed `crashed` from a previous instance must
    // be dropped without breaking the live session.
    let (ws_url, http_url, _store) = start_test_server().await;

    let (mut socket, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade should succeed");
    send_hello(&mut socket, "race-machine").await;
    let _initial = read_json_frame(&mut socket).await; // drain initial empty target

    // Bridge starts agent X with pid 100.
    let started_old = json!({
        "v": 1, "type": "agent.state",
        "data": { "agent_id": "agt-race", "state": "started", "runtime_pid": 100 }
    });
    socket
        .send(Message::Text(started_old.to_string().into()))
        .await
        .unwrap();

    // Bridge restarts agent X — new pid 200.
    let started_new = json!({
        "v": 1, "type": "agent.state",
        "data": { "agent_id": "agt-race", "state": "started", "runtime_pid": 200 }
    });
    socket
        .send(Message::Text(started_new.to_string().into()))
        .await
        .unwrap();

    // Delayed `crashed` from the OLD pid arrives — slice 3's filter
    // must drop it without disturbing the session.
    let stale_crash = json!({
        "v": 1, "type": "agent.state",
        "data": { "agent_id": "agt-race", "state": "crashed",
                  "runtime_pid": 100, "reason": "delayed report from prior instance" }
    });
    socket
        .send(Message::Text(stale_crash.to_string().into()))
        .await
        .unwrap();

    // A `crashed` for the CURRENT pid is accepted (and currently
    // logged; no DB persistence yet).
    let current_crash = json!({
        "v": 1, "type": "agent.state",
        "data": { "agent_id": "agt-race", "state": "crashed",
                  "runtime_pid": 200, "reason": "real crash" }
    });
    socket
        .send(Message::Text(current_crash.to_string().into()))
        .await
        .unwrap();

    // Verify the session is still alive end-to-end by triggering a
    // push-on-change via HTTP and asserting the resulting bridge.target
    // arrives. If any of the four agent.state frames had broken the
    // session loop, this read would time out.
    let client = reqwest::Client::new();
    client
        .post(format!("{http_url}/api/agents"))
        .json(&json!({
            "name": "post-race-bot",
            "display_name": "Post Race Bot",
            "runtime": "claude",
            "model": "sonnet"
        }))
        .send()
        .await
        .expect("POST /api/agents")
        .error_for_status()
        .unwrap();

    let pushed = read_json_frame(&mut socket).await;
    assert_eq!(pushed["type"], "bridge.target");
    assert_eq!(
        pushed["data"]["target_agents"].as_array().unwrap().len(),
        1,
        "session survived stale + current agent.state frames"
    );
}

#[tokio::test]
async fn bridge_ws_pushes_to_multiple_connected_bridges() {
    let (ws_url, http_url, _store) = start_test_server().await;

    let (mut socket_a, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade A");
    send_hello(&mut socket_a, "machine-a").await;
    let _ = read_json_frame(&mut socket_a).await; // drain initial

    let (mut socket_b, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade B");
    send_hello(&mut socket_b, "machine-b").await;
    let _ = read_json_frame(&mut socket_b).await; // drain initial

    // Trigger one CRUD; both bridges should see the same pushed target.
    let client = reqwest::Client::new();
    client
        .post(format!("{http_url}/api/agents"))
        .json(&json!({
            "name": "shared-bot",
            "display_name": "Shared Bot",
            "runtime": "claude",
            "model": "sonnet"
        }))
        .send()
        .await
        .expect("POST /api/agents")
        .error_for_status()
        .unwrap();

    let pushed_a = read_json_frame(&mut socket_a).await;
    let pushed_b = read_json_frame(&mut socket_b).await;
    assert_eq!(pushed_a["type"], "bridge.target");
    assert_eq!(pushed_b["type"], "bridge.target");
    assert_eq!(
        pushed_a["data"]["target_agents"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        pushed_b["data"]["target_agents"].as_array().unwrap().len(),
        1
    );
}
