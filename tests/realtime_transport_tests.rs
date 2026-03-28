use chorus::server::transport::realtime::event_to_json_value;
use chorus::store::StoredEvent;
use serde_json::json;

#[test]
fn test_event_to_json_value_uses_transport_contract_shape() {
    let event = StoredEvent {
        event_id: 42,
        event_type: "message.created".to_string(),
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

    let value = event_to_json_value(&event);
    assert_eq!(value["eventId"], 42);
    assert_eq!(value["eventType"], "message.created");
    assert_eq!(value["scopeKind"], "channel");
    assert_eq!(value["actor"]["name"], "alice");
    assert_eq!(value["causedBy"]["kind"], "send_message");
    assert_eq!(value["payload"]["content"], "hello");
}
