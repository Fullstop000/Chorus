use chorus::server::transport::realtime::event_to_json_value;
use chorus::store::channels::ChannelType;
use chorus::store::messages::SenderType;
use chorus::store::{Store, StoredEvent};
use rusqlite::params;
use serde_json::json;

#[test]
fn test_event_to_json_value_uses_transport_contract_shape() {
    let store = Store::open(":memory:").unwrap();
    let event = StoredEvent {
        event_id: 42,
        event_type: "message.created".to_string(),
        stream_id: "conversation:abc".to_string(),
        stream_kind: "conversation".to_string(),
        stream_pos: 7,
        scope_kind: "channel".to_string(),
        scope_id: "channel:abc".to_string(),
        channel_id: Some("abc".to_string()),
        channel_name: Some("general".to_string()),
        thread_parent_id: None,
        actor_name: Some("alice".to_string()),
        actor_type: Some("human".to_string()),
        caused_by_kind: Some("send_message".to_string()),
        payload: json!({ "content": "hello" }),
        created_at: chrono::DateTime::parse_from_rfc3339("2026-03-28T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    };

    let value = event_to_json_value(&store, &event);
    assert_eq!(value["eventId"], 42);
    assert_eq!(value["eventType"], "message.created");
    assert_eq!(value["streamId"], "conversation:abc");
    assert_eq!(value["streamKind"], "conversation");
    assert_eq!(value["streamPos"], 7);
    assert_eq!(value["scopeKind"], "channel");
    assert_eq!(value["actor"]["name"], "alice");
    assert_eq!(value["causedBy"]["kind"], "send_message");
    assert_eq!(value["payload"]["content"], "hello");
}

#[test]
fn test_event_to_json_value_rehydrates_message_created_payload_from_store_projection() {
    let store = Store::open(":memory:").unwrap();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let message_id = store
        .send_message("general", None, "alice", SenderType::Human, "hello", &[])
        .unwrap();

    {
        let conn = store.conn_for_test();
        conn.execute(
            "UPDATE events
             SET payload = ?1
             WHERE event_type = 'message.created' AND event_id = 1",
            params![json!({
                "messageId": message_id,
                "content": "stale-content"
            })
            .to_string()],
        )
        .unwrap();
    }

    let event = store.list_events(None, 1).unwrap().remove(0);
    let value = event_to_json_value(&store, &event);

    assert_eq!(value["eventType"], "message.created");
    assert_eq!(value["payload"]["messageId"], message_id);
    assert_eq!(value["payload"]["content"], "hello");
    assert_eq!(value["payload"]["sender"]["name"], "alice");
    assert_eq!(value["payload"]["senderDeleted"], false);
    assert_eq!(value["payload"]["seq"], 1);
}
