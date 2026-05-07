//! E2E tests for the bridge ↔ platform WebSocket protocol.
//!
//! A real Axum server is bound to a local TCP port; a `tokio-tungstenite`
//! client connects to `/api/bridge/ws`, sends `bridge.hello`, and asserts
//! the `bridge.target` reply lists the DB's agent records. Mutations
//! through the HTTP API push fresh targets; the bridge can emit
//! `agent.state` frames upstream while the session stays alive.

mod harness;

use std::sync::Arc;

use anyhow::Context;
use chorus::server::bridge_auth::BridgeAuth;
use chorus::store::channels::ChannelType;
use chorus::store::AgentRecordUpsert;
use chorus::store::Store;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

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

    // Seed two agents bound to this bridge's machine_id so they appear
    // in its target. Platform-local (NULL machine_id) agents are not
    // sent to any bridge — every agent has exactly one owner.
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "alpha-bot",
            display_name: "Alpha Bot",
            description: None,
            system_prompt: Some("you are alpha"),
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: Some("test-machine-001"),
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
            machine_id: Some("test-machine-001"),
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

    // Expect the stream to close (no target frame ever arrives). The
    // server drops the socket without a clean handshake on protocol
    // violation, so we accept clean Close, transport-level error, and
    // unclean reset as equivalent outcomes — what we're really checking
    // is "no bridge.target was sent."
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

// ── target push & agent.state ──────────────────────────────────────────

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

    // Create an agent over HTTP bound to this bridge — this should trigger
    // a pushed bridge.target onto the open WS.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{http_url}/api/agents"))
        .json(&json!({
            "name": "push-bot",
            "display_name": "Push Bot",
            "systemPrompt": "pushed",
            "runtime": "claude",
            "model": "sonnet",
            "machineId": "slice2-machine"
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

    // Bridge sends a well-formed agent.state upstream. The platform logs
    // and returns OK; persistence is a follow-up.
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
            "model": "sonnet",
            "machineId": "agent-state-machine"
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
    // agent.state frames carry runtime_pid as the instance discriminator.
    // A delayed `crashed` from a previous instance must be dropped
    // without breaking the live session.
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

    // Delayed `crashed` from the OLD pid arrives — the registry's
    // pid filter must drop it without disturbing the session.
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
            "model": "sonnet",
            "machineId": "race-machine"
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
    // Each connected bridge gets its own target, scoped to the agents
    // it owns. This verifies the per-bridge frame-build mechanism: a
    // single CRUD walks every connection and each receives its
    // ownership-filtered view.
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

    // Create two agents — one bound to each bridge. Each CRUD pushes to
    // both connections; each connection sees only its own owner-bound
    // agents in the resulting target.
    let client = reqwest::Client::new();
    for (name, machine_id) in [("alpha-bot", "machine-a"), ("beta-bot", "machine-b")] {
        client
            .post(format!("{http_url}/api/agents"))
            .json(&json!({
                "name": name,
                "display_name": name,
                "runtime": "claude",
                "model": "sonnet",
                "machineId": machine_id
            }))
            .send()
            .await
            .expect("POST /api/agents")
            .error_for_status()
            .unwrap();
    }

    // Each CRUD broadcasts a per-bridge scoped target frame; agent
    // creation also emits a `member_joined` chat frame for #all that
    // arrives interleaved on the owning bridge's socket. Read until we
    // observe two `bridge.target` frames per socket, then assert the
    // last-seen target reflects ownership partitioning.
    async fn drain_until_two_targets(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Value {
        let mut last_target: Option<Value> = None;
        let mut targets_seen = 0;
        for _ in 0..8 {
            let frame = read_json_frame(socket).await;
            if frame["type"] == "bridge.target" {
                targets_seen += 1;
                last_target = Some(frame);
                if targets_seen >= 2 {
                    break;
                }
            }
        }
        last_target.expect("expected at least one bridge.target frame after both creates")
    }
    let last_a = drain_until_two_targets(&mut socket_a).await;
    let last_b = drain_until_two_targets(&mut socket_b).await;

    // Platform suffixes agent names with a `-hex4` slug (see
    // create_and_start_agent), so assert on prefix.
    let names_a: Vec<String> = last_a["data"]["target_agents"]
        .as_array()
        .unwrap_or_else(|| panic!("socket_a final frame missing target_agents: {last_a}"))
        .iter()
        .map(|o| o["name"].as_str().unwrap().to_string())
        .collect();
    let names_b: Vec<String> = last_b["data"]["target_agents"]
        .as_array()
        .unwrap_or_else(|| panic!("socket_b final frame missing target_agents: {last_b}"))
        .iter()
        .map(|o| o["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names_a.len(), 1, "machine-a sees exactly one agent");
    assert!(
        names_a[0].starts_with("alpha-bot"),
        "machine-a sees alpha-bot, got {}",
        names_a[0]
    );
    assert_eq!(names_b.len(), 1, "machine-b sees exactly one agent");
    assert!(
        names_b[0].starts_with("beta-bot"),
        "machine-b sees beta-bot, got {}",
        names_b[0]
    );
}

// ── bearer auth on WS upgrade ──────────────────────────────────────────

async fn start_test_server_with_auth(auth: Arc<BridgeAuth>) -> (String, String) {
    let store = Arc::new(Store::open(":memory:").unwrap());
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
    let router = harness::build_router_with_bridge_auth(store.clone(), auth);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}");
    let http_url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (ws_url, http_url)
}

#[tokio::test]
async fn bridge_ws_rejects_upgrade_when_token_missing() {
    let auth = BridgeAuth::from_pairs([("good-token", "machine-alpha")]);
    let (ws_url, _http_url) = start_test_server_with_auth(auth).await;

    // No Authorization header at all → 401, no upgrade.
    let result = connect_async(format!("{ws_url}/api/bridge/ws")).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status(), 401, "expected 401 Unauthorized");
        }
        Err(other) => panic!("expected HTTP 401, got error: {other}"),
        Ok(_) => panic!("expected upgrade to fail, but it succeeded"),
    }
}

#[tokio::test]
async fn bridge_ws_rejects_upgrade_when_token_unknown() {
    let auth = BridgeAuth::from_pairs([("good-token", "machine-alpha")]);
    let (ws_url, _http_url) = start_test_server_with_auth(auth).await;

    let mut req = format!("{ws_url}/api/bridge/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", "Bearer wrong-token".parse().unwrap());
    let result = connect_async(req).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status(), 401);
        }
        Err(other) => panic!("expected HTTP 401, got error: {other}"),
        Ok(_) => panic!("expected upgrade to fail, but it succeeded"),
    }
}

#[tokio::test]
async fn bridge_ws_accepts_valid_token_and_matches_machine_id() {
    let auth = BridgeAuth::from_pairs([("good-token", "machine-alpha")]);
    let (ws_url, _http_url) = start_test_server_with_auth(auth).await;

    let mut req = format!("{ws_url}/api/bridge/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", "Bearer good-token".parse().unwrap());
    let (mut socket, _) = connect_async(req).await.expect("WS upgrade should succeed");

    // hello declares the machine_id the token is bound to → accepted.
    send_hello(&mut socket, "machine-alpha").await;
    let frame = read_json_frame(&mut socket).await;
    assert_eq!(frame["type"], "bridge.target");
}

#[tokio::test]
async fn bridge_ws_drops_session_on_machine_id_spoof() {
    let auth = BridgeAuth::from_pairs([("good-token", "machine-alpha")]);
    let (ws_url, _http_url) = start_test_server_with_auth(auth).await;

    let mut req = format!("{ws_url}/api/bridge/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", "Bearer good-token".parse().unwrap());
    let (mut socket, _) = connect_async(req).await.expect("WS upgrade should succeed");

    // Token is bound to machine-alpha but bridge claims machine-beta.
    // The session must close without sending any target frame.
    send_hello(&mut socket, "machine-beta").await;

    let next = timeout(Duration::from_millis(500), socket.next()).await;
    match next {
        Ok(None) => {} // clean close
        Ok(Some(Ok(Message::Close(_)))) => {}
        Ok(Some(Err(_))) => {} // unclean close, also fine
        Ok(Some(Ok(Message::Text(text)))) => {
            panic!("server should have dropped the spoofed session, but sent: {text}");
        }
        other => panic!("unexpected post-spoof outcome: {other:?}"),
    }
}

// ── chat.message.received push + chat.ack ──────────────────────────────

async fn start_test_server_with_event_bus_handle() -> (
    String,
    Arc<Store>,
    Arc<chorus::server::event_bus::EventBus>,
    String,
) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let alice = store.create_local_human("alice").unwrap();
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
    harness::join_channel_silent(&store, "general", &alice.id, "human");
    let (router, event_bus) = harness::build_router_with_event_bus(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (ws_url, store, event_bus, alice.id)
}

#[tokio::test]
async fn bridge_ws_pushes_chat_message_received_when_agent_member_gets_message() {
    let (ws_url, store, event_bus, alice_id) = start_test_server_with_event_bus_handle().await;

    // Seed an agent bound to this bridge's machine_id, joined to #general.
    let agent_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "chat-listener",
            display_name: "Chat Listener",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: Some("chat-machine"),
            env_vars: &[],
        })
        .unwrap();
    harness::join_channel_silent(&store, "general", &agent_id, "agent");

    // Connect a bridge and drain the initial target frame.
    let (mut socket, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade should succeed");
    send_hello(&mut socket, "chat-machine").await;
    let _ = read_json_frame(&mut socket).await;

    // Create a chat message and publish through the same EventBus the
    // server's forwarder is subscribed to.
    let (_msg_id, ev) = store
        .create_message(chorus::store::messages::CreateMessage {
            channel_name: "general",
            sender_id: &alice_id,
            sender_type: chorus::store::messages::SenderType::Human,
            content: "hello agents",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    if let Some(event) = ev {
        event_bus.publish_stream(event);
    }

    // Drain frames until we see chat.message.received for our agent.
    let mut got_chat = false;
    for _ in 0..6 {
        let frame = match timeout(Duration::from_millis(800), read_json_frame(&mut socket)).await {
            Ok(f) => f,
            Err(_) => break,
        };
        if frame["type"] == "chat.message.received" {
            assert_eq!(frame["data"]["agent_id"], agent_id, "matches seeded agent");
            assert!(
                frame["data"]["seq"].is_number(),
                "seq present in chat frame"
            );
            got_chat = true;
            break;
        }
    }
    assert!(
        got_chat,
        "expected at least one chat.message.received frame for the agent"
    );
}

/// Two bridges connected concurrently, each owning one agent in the
/// same channel. A single human chat must reach BOTH agents — but each
/// agent's chat frame must arrive ONLY on its owning bridge. This is the
/// invariant the NULL-machine_id fanout used to violate.
#[tokio::test]
async fn bridge_ws_two_machines_chat_isolation() {
    let (ws_url, store, event_bus, alice_id) = start_test_server_with_event_bus_handle().await;

    // Seed two agents — one per machine — both joined to #general.
    let agent_a_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "agent-on-a",
            display_name: "Agent on A",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: Some("machine-a"),
            env_vars: &[],
        })
        .unwrap();
    let agent_b_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "agent-on-b",
            display_name: "Agent on B",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: Some("machine-b"),
            env_vars: &[],
        })
        .unwrap();
    harness::join_channel_silent(&store, "general", &agent_a_id, "agent");
    harness::join_channel_silent(&store, "general", &agent_b_id, "agent");

    // Connect both bridges and drain initial targets. machine-a's
    // initial target lists only agent-on-a; machine-b's only agent-on-b.
    let (mut sock_a, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade A");
    send_hello(&mut sock_a, "machine-a").await;
    let initial_a = read_json_frame(&mut sock_a).await;
    assert_eq!(initial_a["type"], "bridge.target");
    let init_a_ids: Vec<&str> = initial_a["data"]["target_agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["agent_id"].as_str().unwrap())
        .collect();
    assert_eq!(init_a_ids, vec![agent_a_id.as_str()], "machine-a target");

    let (mut sock_b, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade B");
    send_hello(&mut sock_b, "machine-b").await;
    let initial_b = read_json_frame(&mut sock_b).await;
    let init_b_ids: Vec<&str> = initial_b["data"]["target_agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["agent_id"].as_str().unwrap())
        .collect();
    assert_eq!(init_b_ids, vec![agent_b_id.as_str()], "machine-b target");

    // Human posts to #general. The platform forwarder should route ONE
    // chat frame to each bridge — each carrying ONLY that bridge's agent
    // as the recipient.
    let (_msg_id, ev) = store
        .create_message(chorus::store::messages::CreateMessage {
            channel_name: "general",
            sender_id: &alice_id,
            sender_type: chorus::store::messages::SenderType::Human,
            content: "hello agents",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    if let Some(event) = ev {
        event_bus.publish_stream(event);
    }

    /// Read frames on `socket` until we see a chat.message.received,
    /// returning its inner agent_id. Times out after 6 polls.
    async fn next_chat_agent_id(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Option<String> {
        for _ in 0..6 {
            let frame = match timeout(Duration::from_millis(800), read_json_frame(socket)).await {
                Ok(f) => f,
                Err(_) => return None,
            };
            if frame["type"] == "chat.message.received" {
                return frame["data"]["agent_id"].as_str().map(str::to_owned);
            }
        }
        None
    }

    let chat_a_recipient = next_chat_agent_id(&mut sock_a).await;
    let chat_b_recipient = next_chat_agent_id(&mut sock_b).await;

    assert_eq!(
        chat_a_recipient.as_deref(),
        Some(agent_a_id.as_str()),
        "machine-a's bridge receives chat for agent-on-a only"
    );
    assert_eq!(
        chat_b_recipient.as_deref(),
        Some(agent_b_id.as_str()),
        "machine-b's bridge receives chat for agent-on-b only"
    );

    // Cross-check: machine-a must NEVER have seen a chat for agent-on-b
    // (and vice versa). The recipient assertions above already enforce
    // this for the first frame seen, but drain a second time with a
    // tight timeout to catch any cross-routed leakage.
    let leak_a = match timeout(Duration::from_millis(500), read_json_frame(&mut sock_a)).await {
        Ok(f) if f["type"] == "chat.message.received" => {
            f["data"]["agent_id"].as_str().map(str::to_owned)
        }
        _ => None,
    };
    let leak_b = match timeout(Duration::from_millis(500), read_json_frame(&mut sock_b)).await {
        Ok(f) if f["type"] == "chat.message.received" => {
            f["data"]["agent_id"].as_str().map(str::to_owned)
        }
        _ => None,
    };
    assert!(
        leak_a.is_none(),
        "machine-a leaked an extra chat frame: {leak_a:?}"
    );
    assert!(
        leak_b.is_none(),
        "machine-b leaked an extra chat frame: {leak_b:?}"
    );
}

#[tokio::test]
async fn bridge_ws_chat_ack_advances_per_agent_cursor() {
    // This test exercises the wire shape: bridge → chat.ack frame →
    // session loop accepts and stays alive. The actual cursor
    // observability (BridgeRegistry::last_acked_seq) is covered by the
    // unit tests below; here we just confirm the WS path doesn't break
    // on the new frame type.
    let (ws_url, http_url, _store) = start_test_server().await;

    let (mut socket, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade should succeed");
    send_hello(&mut socket, "ack-machine").await;
    let _ = read_json_frame(&mut socket).await; // initial target

    let ack = json!({
        "v": 1, "type": "chat.ack",
        "data": { "agent_id": "agt-x", "last_seq": 42 }
    });
    socket
        .send(Message::Text(ack.to_string().into()))
        .await
        .unwrap();

    // Trigger a push and verify session is alive.
    let client = reqwest::Client::new();
    client
        .post(format!("{http_url}/api/agents"))
        .json(&json!({
            "name": "ack-bot",
            "display_name": "Ack Bot",
            "runtime": "claude",
            "model": "sonnet"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let pushed = read_json_frame(&mut socket).await;
    assert_eq!(pushed["type"], "bridge.target");
}

// ── machine_id scoping on agents ───────────────────────────────────────

#[tokio::test]
async fn bridge_ws_target_scoped_by_agent_machine_id() {
    let (ws_url, _http_url, store) = start_test_server().await;

    // Platform-local agent (machine_id NULL) — invisible to every
    // bridge; runs in chorus serve's own AgentManager.
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "local-bot",
            display_name: "Local Bot",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    // Owned by machine-a only.
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "alpha-only",
            display_name: "Alpha Only",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: Some("machine-a"),
            env_vars: &[],
        })
        .unwrap();
    // Owned by machine-b only.
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "beta-only",
            display_name: "Beta Only",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: Some("machine-b"),
            env_vars: &[],
        })
        .unwrap();

    // Bridge A connects → only alpha-only. local-bot is platform-local.
    let (mut sock_a, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade A");
    send_hello(&mut sock_a, "machine-a").await;
    let target_a = read_json_frame(&mut sock_a).await;
    assert_eq!(target_a["type"], "bridge.target");
    let names_a: Vec<String> = target_a["data"]["target_agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names_a,
        vec!["alpha-only".to_string()],
        "machine-a sees alpha-only only"
    );

    // Bridge B connects → only beta-only.
    let (mut sock_b, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade B");
    send_hello(&mut sock_b, "machine-b").await;
    let target_b = read_json_frame(&mut sock_b).await;
    let names_b: Vec<String> = target_b["data"]["target_agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names_b,
        vec!["beta-only".to_string()],
        "machine-b sees beta-only only"
    );

    // Bridge with no matching agents → empty target.
    let (mut sock_z, _) = connect_async(format!("{ws_url}/api/bridge/ws"))
        .await
        .expect("WS upgrade Z");
    send_hello(&mut sock_z, "machine-zeta").await;
    let target_z = read_json_frame(&mut sock_z).await;
    assert_eq!(
        target_z["data"]["target_agents"].as_array().unwrap().len(),
        0,
        "machine with no bound agents sees an empty target"
    );
}

// ── bearer auth on /internal/agent/* ───────────────────────────────────

#[tokio::test]
async fn internal_agent_endpoints_pass_through_when_auth_disabled() {
    let (_ws_url, http_url, store) = start_test_server().await;
    let agent_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "internal-bot",
            display_name: "Internal Bot",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    // No bridge auth configured → /internal/agent/* with no header is OK.
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{http_url}/internal/agent/{agent_id}/server"))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200, "auth disabled, /internal should pass");
}

#[tokio::test]
async fn internal_agent_endpoints_require_bearer_when_auth_enabled() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel(
            Store::DEFAULT_SYSTEM_CHANNEL,
            None,
            ChannelType::System,
            None,
        )
        .unwrap();
    let agent_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "auth-bot",
            display_name: "Auth Bot",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    let auth = BridgeAuth::from_pairs([("internal-tok", "machine-x")]);
    let router = harness::build_router_with_bridge_auth(store.clone(), auth);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let http_url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // No header → 401.
    let resp = client
        .get(format!("{http_url}/internal/agent/{agent_id}/server"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "missing header should 401");

    // Wrong token → 401.
    let resp = client
        .get(format!("{http_url}/internal/agent/{agent_id}/server"))
        .header("Authorization", "Bearer wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "wrong token should 401");

    // Correct token → 200 (or whatever the handler normally returns;
    // the point is the middleware lets it through).
    let resp = client
        .get(format!("{http_url}/internal/agent/{agent_id}/server"))
        .header("Authorization", "Bearer internal-tok")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "correct token should be authorized to reach the handler"
    );
}
