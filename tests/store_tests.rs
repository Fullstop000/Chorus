use chorus::store::Store;
use chorus::models::*;
use tempfile::tempdir;

fn make_store() -> (Store, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Store::open(db_path.to_str().unwrap()).unwrap();
    (store, dir)
}

#[test]
fn test_create_and_list_channels() {
    let (store, _dir) = make_store();
    store.create_channel("general", Some("General channel"), ChannelType::Channel).unwrap();
    store.create_channel("random", None, ChannelType::Channel).unwrap();
    let channels = store.list_channels().unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].name, "general");
}

#[test]
fn test_send_and_receive_messages() {
    let (store, _dir) = make_store();
    store.create_channel("general", None, ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.join_channel("general", "alice", SenderType::Human).unwrap();

    let msg_id = store.send_message("general", None, "alice", SenderType::Human, "hello", &[]).unwrap();
    assert!(!msg_id.is_empty());

    let msgs = store.get_messages_for_agent("bob", false).unwrap();
    assert!(msgs.is_empty());

    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.join_channel("general", "bot1", SenderType::Agent).unwrap();

    let _msg_id2 = store.send_message("general", None, "alice", SenderType::Human, "hello bot", &[]).unwrap();
    let msgs = store.get_messages_for_agent("bot1", false).unwrap();
    assert_eq!(msgs.len(), 2);
}

#[test]
fn test_message_history_pagination() {
    let (store, _dir) = make_store();
    store.create_channel("general", None, ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.join_channel("general", "alice", SenderType::Human).unwrap();

    for i in 0..10 {
        store.send_message("general", None, "alice", SenderType::Human, &format!("msg {i}"), &[]).unwrap();
    }

    let (msgs, has_more) = store.get_history("general", None, 5, None, None).unwrap();
    assert_eq!(msgs.len(), 5);
    assert!(has_more);

    let first_seq = msgs[0].seq;
    let (older, _) = store.get_history("general", None, 5, Some(first_seq), None).unwrap();
    assert_eq!(older.len(), 5);
}

#[test]
fn test_tasks_crud() {
    let (store, _dir) = make_store();
    store.create_channel("eng", None, ChannelType::Channel).unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();

    let tasks = store.create_tasks("eng", "bot1", &["Fix bug", "Add feature"]).unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].task_number, 1);
    assert_eq!(tasks[1].task_number, 2);

    let listed = store.list_tasks("eng", None).unwrap();
    assert_eq!(listed.len(), 2);
}

#[test]
fn test_task_claim_and_status() {
    let (store, _dir) = make_store();
    store.create_channel("eng", None, ChannelType::Channel).unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.create_agent_record("bot2", "Bot 2", None, "codex", "o3").unwrap();
    store.create_tasks("eng", "bot1", &["Task A"]).unwrap();

    let results = store.claim_tasks("eng", "bot1", &[1]).unwrap();
    assert!(results[0].success);

    let results = store.claim_tasks("eng", "bot2", &[1]).unwrap();
    assert!(!results[0].success);

    store.update_task_status("eng", 1, "bot1", TaskStatus::InReview).unwrap();
    let tasks = store.list_tasks("eng", Some(TaskStatus::InReview)).unwrap();
    assert_eq!(tasks.len(), 1);
}

#[test]
fn test_resolve_target() {
    let (store, _dir) = make_store();
    store.create_channel("general", None, ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();

    let (ch_id, thread_parent) = store.resolve_target("#general", "bot1").unwrap();
    assert!(!ch_id.is_empty());
    assert!(thread_parent.is_none());

    let (dm_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    assert!(!dm_id.is_empty());
}

#[test]
fn test_list_channels_excludes_dm() {
    let (store, _dir) = make_store();
    store.create_channel("general", None, ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    // Create a DM channel via resolve_target
    store.resolve_target("dm:@alice", "bot1").unwrap();

    let channels = store.list_channels().unwrap();
    assert_eq!(channels.len(), 1, "list_channels must not return DM channels");
    assert_eq!(channels[0].name, "general");
}

#[test]
fn test_dm_only_has_two_members() {
    let (store, _dir) = make_store();
    store.create_channel("general", None, ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.create_agent_record("bot2", "Bot 2", None, "claude", "sonnet").unwrap();

    // Create DM between alice and bot1
    let (dm_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();

    // Simulate old bug: manually add bot2 as a spurious member
    store.join_channel("dm-alice-bot1", "bot2", SenderType::Agent).unwrap();

    let members = store.get_channel_members(&dm_id).unwrap();
    assert_eq!(members.len(), 3, "precondition: spurious member added");

    // Re-open store to trigger the migration
    let db_path = _dir.path().join("test.db");
    let store2 = Store::open(db_path.to_str().unwrap()).unwrap();
    let members2 = store2.get_channel_members(&dm_id).unwrap();
    assert_eq!(members2.len(), 2, "migration must remove spurious DM members");
    let names: Vec<_> = members2.iter().map(|m| m.member_name.as_str()).collect();
    assert!(names.contains(&"alice"), "alice must remain");
    assert!(names.contains(&"bot1"), "bot1 must remain");
}

#[test]
fn test_dm_channels() {
    let (store, _dir) = make_store();
    store.add_human("alice").unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();

    let (ch_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    let (ch_id2, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    assert_eq!(ch_id, ch_id2);
}
