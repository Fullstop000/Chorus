use std::sync::Arc;

use anyhow::Context;
use chorus::server::build_router;
use chorus::store::channels::ChannelType;
use chorus::store::messages::SenderType;
use chorus::store::Store;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::{connect_async, tungstenite::Message};

async fn start_test_server() -> (String, Arc<Store>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.add_human("alice").unwrap();
    store.add_human("zoe").unwrap();
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
async fn test_realtime_subscription_replays_matching_events_from_cursor() {
    let (base_url, store) = start_test_server().await;
    let channel_id = store.find_channel_by_name("general").unwrap().unwrap().id;

    store
        .send_message("general", None, "alice", SenderType::Human, "hello", &[])
        .unwrap();

    let (mut socket, _) = connect_async(format!("{base_url}/api/events/ws?viewer=alice"))
        .await
        .unwrap();

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "resumeFrom": 0,
                "scopes": [
                    {
                        "kind": "channel",
                        "id": format!("channel:{channel_id}")
                    }
                ]
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let subscribed = read_json_frame(&mut socket).await;
    assert_eq!(subscribed["type"], "subscribed");

    let event = read_json_frame(&mut socket).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["event"]["eventType"], "message.created");
    assert_eq!(event["event"]["scopeKind"], "channel");
    assert_eq!(event["event"]["payload"]["content"], "hello");
}

#[tokio::test]
async fn test_realtime_subscription_can_resume_from_stream_position() {
    let (base_url, store) = start_test_server().await;
    let general_id = store.find_channel_by_name("general").unwrap().unwrap().id;
    store
        .create_channel("random", Some("Random"), ChannelType::Channel)
        .unwrap();
    store
        .join_channel("random", "alice", SenderType::Human)
        .unwrap();

    store
        .send_message(
            "general",
            None,
            "alice",
            SenderType::Human,
            "general-1",
            &[],
        )
        .unwrap();
    store
        .send_message("random", None, "alice", SenderType::Human, "random-1", &[])
        .unwrap();
    store
        .send_message(
            "general",
            None,
            "alice",
            SenderType::Human,
            "general-2",
            &[],
        )
        .unwrap();

    let (mut socket, _) = connect_async(format!("{base_url}/api/events/ws?viewer=alice"))
        .await
        .unwrap();

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "resumeFrom": 999,
                "streamId": format!("conversation:{general_id}"),
                "resumeFromStreamPos": 1,
                "scopes": [
                    {
                        "kind": "channel",
                        "id": format!("channel:{general_id}")
                    }
                ]
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let subscribed = read_json_frame(&mut socket).await;
    assert_eq!(subscribed["type"], "subscribed");
    assert_eq!(
        subscribed["streamId"],
        Value::String(format!("conversation:{general_id}"))
    );
    assert_eq!(subscribed["resumeFromStreamPos"], 1);

    let event = read_json_frame(&mut socket).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["event"]["eventType"], "message.created");
    assert_eq!(
        event["event"]["streamId"],
        format!("conversation:{general_id}")
    );
    assert_eq!(event["event"]["streamPos"], 2);
    assert_eq!(event["event"]["payload"]["content"], "general-2");
}

#[tokio::test]
async fn test_realtime_subscription_receives_live_events_after_subscribe() {
    let (base_url, store) = start_test_server().await;
    let channel_id = store.find_channel_by_name("general").unwrap().unwrap().id;

    let (mut socket, _) = connect_async(format!("{base_url}/api/events/ws?viewer=alice"))
        .await
        .unwrap();

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "resumeFrom": 0,
                "scopes": [
                    {
                        "kind": "channel",
                        "id": format!("channel:{channel_id}")
                    }
                ]
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let subscribed = read_json_frame(&mut socket).await;
    assert_eq!(subscribed["type"], "subscribed");

    store
        .send_message("general", None, "alice", SenderType::Human, "live", &[])
        .unwrap();

    let event = read_json_frame(&mut socket).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["event"]["eventType"], "message.created");
    assert_eq!(event["event"]["payload"]["content"], "live");
}

#[tokio::test]
async fn test_additive_subscribe_across_conversations_keeps_global_live_delivery() {
    let (base_url, store) = start_test_server().await;
    let general_id = store.find_channel_by_name("general").unwrap().unwrap().id;
    let random_id = store
        .create_channel("random", Some("Random"), ChannelType::Channel)
        .unwrap();
    store
        .join_channel("random", "alice", SenderType::Human)
        .unwrap();

    store
        .send_message(
            "general",
            None,
            "alice",
            SenderType::Human,
            "general-seed",
            &[],
        )
        .unwrap();
    store
        .send_message(
            "random",
            None,
            "alice",
            SenderType::Human,
            "random-seed",
            &[],
        )
        .unwrap();

    let (mut socket, _) = connect_async(format!("{base_url}/api/events/ws?viewer=alice"))
        .await
        .unwrap();

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "resumeFrom": 2,
                "streamId": format!("conversation:{general_id}"),
                "resumeFromStreamPos": 1,
                "scopes": [
                    {
                        "kind": "channel",
                        "id": format!("channel:{general_id}")
                    }
                ]
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let first_subscribed = read_json_frame(&mut socket).await;
    assert_eq!(first_subscribed["type"], "subscribed");
    assert_eq!(
        first_subscribed["streamId"],
        Value::String(format!("conversation:{general_id}"))
    );

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "resumeFrom": 2,
                "scopes": [
                    {
                        "kind": "channel",
                        "id": format!("channel:{random_id}")
                    }
                ]
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let second_subscribed = read_json_frame(&mut socket).await;
    assert_eq!(second_subscribed["type"], "subscribed");
    assert_eq!(second_subscribed["streamId"], Value::Null);

    store
        .send_message(
            "general",
            None,
            "alice",
            SenderType::Human,
            "general-live-after-multi-subscribe",
            &[],
        )
        .unwrap();

    let event = read_json_frame(&mut socket).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["event"]["scopeKind"], "channel");
    assert_eq!(
        event["event"]["scopeId"],
        Value::String(format!("channel:{general_id}"))
    );
    assert_eq!(
        event["event"]["payload"]["content"],
        "general-live-after-multi-subscribe"
    );
}

#[tokio::test]
async fn test_realtime_subscription_rejects_forbidden_scope() {
    let (base_url, store) = start_test_server().await;
    let private_channel_id = store
        .create_channel("private", Some("Private"), ChannelType::Channel)
        .unwrap();
    store
        .join_channel("private", "zoe", SenderType::Human)
        .unwrap();

    let (mut socket, _) = connect_async(format!("{base_url}/api/events/ws?viewer=alice"))
        .await
        .unwrap();

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "resumeFrom": 0,
                "scopes": [
                    {
                        "kind": "channel",
                        "id": format!("channel:{private_channel_id}")
                    }
                ]
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let error = read_json_frame(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["code"], "forbidden_scope");
}
