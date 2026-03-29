use chorus::server::transport::realtime::event_to_json_value;
use chorus::store::channels::ChannelType;
use chorus::store::messages::SenderType;
use chorus::store::{Store, StoredEvent};
use rusqlite::params;
use serde_json::json;

#[test]
fn test_event_to_json_value_uses_notification_contract_shape() {
    let store = Store::open(":memory:").unwrap();
    let event = StoredEvent {
        event_id: 42,
        event_type: "conversation.state".to_string(),
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
        payload: json!({
            "messageId": "msg-1",
            "latestSeq": 7
        }),
        created_at: chrono::DateTime::parse_from_rfc3339("2026-03-28T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    };

    let value = event_to_json_value(&store, &event);
    assert_eq!(value["eventId"], 42);
    assert_eq!(value["eventType"], "conversation.state");
    assert_eq!(value["streamId"], "conversation:abc");
    assert_eq!(value["streamKind"], "conversation");
    assert_eq!(value["streamPos"], 7);
    assert_eq!(value["scopeKind"], "channel");
    assert_eq!(value["actor"]["name"], "alice");
    assert_eq!(value["causedBy"]["kind"], "send_message");
    assert_eq!(value["payload"]["messageId"], "msg-1");
    assert_eq!(value["payload"]["latestSeq"], 7);
    assert!(value["payload"].get("content").is_none());
}

#[test]
fn test_event_to_json_value_uses_conversation_state_without_message_body() {
    let store = Store::open(":memory:").unwrap();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let message_id = store
        .create_message("general", None, "alice", SenderType::Human, "hello", &[])
        .unwrap();

    {
        let conn = store.conn_for_test();
        conn.execute(
            "UPDATE events
             SET payload = ?1
             WHERE event_type = 'conversation.state' AND event_id = 1",
            params![json!({
                "messageId": message_id,
                "latestSeq": 1
            })
            .to_string()],
        )
        .unwrap();
    }

    let event = store.get_events(None, 1).unwrap().remove(0);
    let value = event_to_json_value(&store, &event);

    assert_eq!(value["eventType"], "conversation.state");
    assert_eq!(value["payload"]["messageId"], message_id);
    assert_eq!(value["payload"]["latestSeq"], 1);
    assert_eq!(value["payload"]["lastReadSeq"], 1);
    assert_eq!(value["payload"]["unreadCount"], 0);
    assert!(value["payload"].get("content").is_none());
}
