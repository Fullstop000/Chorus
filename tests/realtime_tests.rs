mod harness;

use std::sync::Arc;

use anyhow::Context;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, SenderType};
use chorus::store::Store;
use futures_util::StreamExt;
use harness::build_router;
use serde_json::Value;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

async fn start_test_server() -> (String, Arc<Store>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.create_human("alice").unwrap();
    store.create_human("zoe").unwrap();
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    let router = build_router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (url, store)
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
    let (base_url, store) = start_test_server().await;
    let general_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    let (mut socket, _) = connect_async(format!("{base_url}/api/events/ws?viewer=alice"))
        .await
        .unwrap();

    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

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
    let (base_url, store) = start_test_server().await;
    store
        .create_channel("private", Some("Private"), ChannelType::Channel)
        .unwrap();
    store
        .join_channel("private", "zoe", SenderType::Human)
        .unwrap();

    let (mut socket, _) = connect_async(format!("{base_url}/api/events/ws?viewer=alice"))
        .await
        .unwrap();

    store
        .create_message(CreateMessage {
            channel_name: "private",
            thread_parent_id: None,
            sender_name: "zoe",
            sender_type: SenderType::Human,
            content: "secret",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let next = timeout(Duration::from_millis(250), socket.next()).await;
    assert!(
        next.is_err(),
        "alice should not receive stream events for channels she is not a member of"
    );
}

#[tokio::test]
async fn test_realtime_member_receives_live_messages_without_subscribe_frame() {
    let (base_url, store) = start_test_server().await;

    let (mut socket, _) = connect_async(format!("{base_url}/api/events/ws?viewer=alice"))
        .await
        .unwrap();

    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "live",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let frame = read_json_frame(&mut socket).await;
    assert_eq!(frame["type"], "event");
    assert_eq!(frame["event"]["eventType"], "message.created");
    assert_eq!(frame["event"]["latestSeq"], 1);
}
