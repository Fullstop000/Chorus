mod harness;

use std::sync::Arc;

use anyhow::Context;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, SenderType};
use chorus::store::Store;
use futures_util::StreamExt;
use harness::join_channel_silent;
use serde_json::Value;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

struct TestIdentities {
    alice_id: String,
    zoe_id: String,
}

async fn start_test_server() -> (
    String,
    Arc<Store>,
    TestIdentities,
    Arc<chorus::server::event_bus::EventBus>,
) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let alice = store.create_local_human("alice").unwrap();
    let zoe = store.create_local_human("zoe").unwrap();
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
    store
        .create_channel("general", Some("General"), ChannelType::Channel, None)
        .unwrap();
    join_channel_silent(&store, "general", &alice.id, "human");
    let (router, event_bus) = harness::build_router_with_event_bus(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (
        url,
        store,
        TestIdentities {
            alice_id: alice.id,
            zoe_id: zoe.id,
        },
        event_bus,
    )
}

async fn read_json_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    let frame = socket
        .next()
        .await
        .context("expected websocket frame")
        .unwrap()
        .context("websocket frame should be ok")
        .unwrap();
    let Message::Text(text) = frame else {
        panic!("expected text websocket frame");
    };
    serde_json::from_str(text.as_str()).unwrap()
}

#[tokio::test]
async fn test_realtime_delivers_message_created_for_joined_channel() {
    let (base_url, store, ids, event_bus) = start_test_server().await;
    let general_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    let (mut socket, _) =
        connect_async(format!("{base_url}/api/events/ws?viewer={}", ids.alice_id))
            .await
            .unwrap();

    let (_, event) = store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: &ids.alice_id,
            sender_type: SenderType::Human,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    if let Some(ev) = event {
        event_bus.publish_stream(ev);
    }

    let frame = read_json_frame(&mut socket).await;
    assert_eq!(frame["type"], "event");
    assert_eq!(frame["event"]["eventType"], "message.created");
    assert_eq!(frame["event"]["channelId"], general_id);
    assert_eq!(frame["event"]["latestSeq"], 1);
    assert_eq!(frame["event"]["schemaVersion"], 1);
    assert!(frame["event"]["payload"]["messageId"].is_string());
}

#[tokio::test]
async fn test_realtime_skips_non_member_channel() {
    let (base_url, store, ids, event_bus) = start_test_server().await;
    store
        .create_channel("private", Some("Private"), ChannelType::Channel, None)
        .unwrap();
    join_channel_silent(&store, "private", &ids.zoe_id, "human");

    let (mut socket, _) =
        connect_async(format!("{base_url}/api/events/ws?viewer={}", ids.alice_id))
            .await
            .unwrap();

    let (_, event) = store
        .create_message(CreateMessage {
            channel_name: "private",
            sender_id: &ids.zoe_id,
            sender_type: SenderType::Human,
            content: "secret",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    if let Some(ev) = event {
        event_bus.publish_stream(ev);
    }

    let next = timeout(Duration::from_millis(250), socket.next()).await;
    assert!(
        next.is_err(),
        "alice should not receive stream events for channels she is not a member of"
    );
}

#[tokio::test]
async fn test_realtime_member_receives_live_messages_without_subscribe_frame() {
    let (base_url, store, ids, event_bus) = start_test_server().await;

    let (mut socket, _) =
        connect_async(format!("{base_url}/api/events/ws?viewer={}", ids.alice_id))
            .await
            .unwrap();

    let (_, event) = store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: &ids.alice_id,
            sender_type: SenderType::Human,
            content: "live",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    if let Some(ev) = event {
        event_bus.publish_stream(ev);
    }

    let frame = read_json_frame(&mut socket).await;
    assert_eq!(frame["type"], "event");
    assert_eq!(frame["event"]["eventType"], "message.created");
    assert_eq!(frame["event"]["latestSeq"], 1);
}
