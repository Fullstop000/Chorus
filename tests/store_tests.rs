use chorus::store::agents::AgentEnvVar;
use chorus::store::channels::ChannelType;
use chorus::store::events::StoredEvent;
use chorus::store::messages::SenderType;
use chorus::store::tasks::TaskStatus;
use chorus::store::{AgentRecordUpsert, Store};
use rusqlite::{params, Connection};
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn test_team_tables_exist() {
    let store = Store::open(":memory:").unwrap();
    let conn = store.conn_for_test();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('teams','team_members','team_task_signals','team_task_quorum')",
        [],
        |r| r.get::<_, i64>(0),
    ).unwrap();
    assert_eq!(count, 4);
}

#[test]
fn test_create_and_get_team() {
    let store = Store::open(":memory:").unwrap();
    let id = store
        .create_team(
            "eng-team",
            "Engineering Team",
            "leader_operators",
            Some("alice"),
        )
        .unwrap();
    let team = store.get_team("eng-team").unwrap().unwrap();
    assert_eq!(team.id, id);
    assert_eq!(team.name, "eng-team");
    assert_eq!(team.display_name, "Engineering Team");
    assert_eq!(team.collaboration_model, "leader_operators");
    assert_eq!(team.leader_agent_name.as_deref(), Some("alice"));
}

#[test]
fn test_add_and_list_team_members() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store
        .add_team_member(&team_id, "alice", "agent", "agent-uuid-1", "operator")
        .unwrap();
    store
        .add_team_member(&team_id, "bob", "human", "bob", "observer")
        .unwrap();
    let members = store.get_team_members(&team_id).unwrap();
    assert_eq!(members.len(), 2);
}

#[test]
fn test_list_teams_for_agent() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store
        .add_team_member(&team_id, "alice", "agent", "agent-uuid-1", "operator")
        .unwrap();
    let teams = store.list_teams_for_agent("alice").unwrap();
    assert_eq!(teams.len(), 1);
    assert_eq!(teams[0].team_name, "eng-team");
}

#[test]
fn test_delete_team_cascades() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store
        .add_team_member(&team_id, "alice", "agent", "uuid-1", "operator")
        .unwrap();
    store.delete_team(&team_id).unwrap();
    assert!(store.get_team("eng-team").unwrap().is_none());
    // member row should be gone
    let conn = store.conn_for_test();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM team_members WHERE team_id = ?1",
            params![team_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

fn make_store() -> (Store, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Store::open(db_path.to_str().unwrap()).unwrap();
    (store, dir)
}

#[test]
fn test_create_and_list_channels() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", Some("General channel"), ChannelType::Channel)
        .unwrap();
    store
        .create_channel("random", None, ChannelType::Channel)
        .unwrap();
    let channels = store.list_channels().unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].name, "general");
}

#[test]
fn test_send_and_receive_messages() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let msg_id = store
        .send_message("general", None, "alice", SenderType::Human, "hello", &[])
        .unwrap();
    assert!(!msg_id.is_empty());

    let msgs = store.get_messages_for_agent("bob", false).unwrap();
    assert!(msgs.is_empty());

    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    let _msg_id2 = store
        .send_message(
            "general",
            None,
            "alice",
            SenderType::Human,
            "hello bot",
            &[],
        )
        .unwrap();
    let msgs = store.get_messages_for_agent("bot1", false).unwrap();
    assert_eq!(msgs.len(), 2);
}

#[test]
fn test_agent_does_not_receive_its_own_sent_message() {
    let (store, _dir) = make_store();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store.add_human("alice").unwrap();
    let (dm_channel_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    let dm_channel = store.find_channel_by_id(&dm_channel_id).unwrap().unwrap();

    store
        .send_message(
            &dm_channel.name,
            None,
            "bot1",
            SenderType::Agent,
            "hello alice",
            &[],
        )
        .unwrap();

    let unread = store.get_messages_for_agent("bot1", false).unwrap();
    assert!(
        unread.is_empty(),
        "an agent should not get its own outbound message back as unread"
    );

    let last_read = store.get_last_read_seq(&dm_channel.name, "bot1").unwrap();
    assert_eq!(last_read, 1, "sender read position should advance on send");
}

#[test]
fn test_message_history_pagination() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    for i in 0..10 {
        store
            .send_message(
                "general",
                None,
                "alice",
                SenderType::Human,
                &format!("msg {i}"),
                &[],
            )
            .unwrap();
    }

    let (msgs, has_more) = store.get_history("general", None, 5, None, None).unwrap();
    assert_eq!(msgs.len(), 5);
    assert!(has_more);

    let first_seq = msgs[0].seq;
    let (older, _) = store
        .get_history("general", None, 5, Some(first_seq), None)
        .unwrap();
    assert_eq!(older.len(), 5);
}

#[test]
fn test_agent_env_vars_persist_in_agent_record() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    let env_vars = vec![
        AgentEnvVar {
            key: "OPENAI_API_KEY".to_string(),
            value: "secret".to_string(),
            position: 0,
        },
        AgentEnvVar {
            key: "DEBUG".to_string(),
            value: "1".to_string(),
            position: 1,
        },
    ];
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &env_vars)
        .unwrap();

    let agent = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(agent.env_vars, env_vars);
}

#[test]
fn test_agent_reasoning_effort_persists_in_agent_record() {
    let (store, _dir) = make_store();
    store
        .create_agent_record_with_reasoning(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            runtime: "codex",
            model: "gpt-5.4-mini",
            reasoning_effort: Some("low"),
            env_vars: &[],
        })
        .unwrap();

    let agent = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(agent.reasoning_effort.as_deref(), Some("low"));

    store
        .update_agent_record_with_reasoning(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            runtime: "codex",
            model: "gpt-5.4-mini",
            reasoning_effort: Some("high"),
            env_vars: &[],
        })
        .unwrap();

    let updated = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(updated.reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn test_mark_agent_messages_deleted_marks_history_rows() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .send_message("general", None, "bot1", SenderType::Agent, "hello", &[])
        .unwrap();

    store.mark_agent_messages_deleted("bot1").unwrap();
    let (history, _) = store.get_history("general", None, 10, None, None).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].sender_deleted);
}

fn payload_field<'a>(event: &'a StoredEvent, key: &str) -> &'a Value {
    event
        .payload
        .get(key)
        .unwrap_or_else(|| panic!("missing payload field {key} in {:?}", event.payload))
}

#[test]
fn test_send_message_emits_message_created_event() {
    let (store, _dir) = make_store();
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

    let events = store.list_events(None, 20).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "message.created");
    assert_eq!(events[0].scope_kind, "channel");
    assert_eq!(events[0].actor_name.as_deref(), Some("alice"));
    assert_eq!(events[0].actor_type.as_deref(), Some("human"));
    assert_eq!(
        payload_field(&events[0], "messageId").as_str(),
        Some(message_id.as_str())
    );
    assert_eq!(payload_field(&events[0], "content").as_str(), Some("hello"));
    assert_eq!(
        payload_field(&events[0], "conversationType").as_str(),
        Some("channel")
    );
}

#[test]
fn test_thread_reply_emits_thread_derived_events() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    let parent_id = store
        .send_message("general", None, "alice", SenderType::Human, "parent", &[])
        .unwrap();
    let reply_id = store
        .send_message(
            "general",
            Some(&parent_id),
            "bot1",
            SenderType::Agent,
            "reply",
            &[],
        )
        .unwrap();

    let events = store.list_events(None, 20).unwrap();
    let event_types: Vec<_> = events.iter().map(|event| event.event_type.as_str()).collect();
    assert_eq!(
        event_types,
        vec![
            "message.created",
            "message.created",
            "thread.reply_count_changed",
            "thread.activity_bumped",
            "thread.participant_added",
        ]
    );

    let reply_event = &events[1];
    assert_eq!(reply_event.scope_kind, "thread");
    assert_eq!(
        payload_field(reply_event, "messageId").as_str(),
        Some(reply_id.as_str())
    );
    assert_eq!(
        payload_field(reply_event, "threadParentId").as_str(),
        Some(parent_id.as_str())
    );

    let reply_count_event = &events[2];
    assert_eq!(reply_count_event.scope_kind, "channel");
    assert_eq!(
        payload_field(reply_count_event, "replyCount").as_i64(),
        Some(1)
    );
}

#[test]
fn test_mark_agent_messages_deleted_emits_tombstone_events() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    let message_id = store
        .send_message("general", None, "bot1", SenderType::Agent, "hello", &[])
        .unwrap();

    store.mark_agent_messages_deleted("bot1").unwrap();

    let events = store.list_events(None, 20).unwrap();
    let tombstone_event = events
        .iter()
        .find(|event| event.event_type == "message.tombstone_changed")
        .expect("expected tombstone event after sender deletion");
    assert_eq!(
        payload_field(tombstone_event, "messageId").as_str(),
        Some(message_id.as_str())
    );
    assert_eq!(
        payload_field(tombstone_event, "senderDeleted").as_bool(),
        Some(true)
    );
}

#[test]
fn test_tasks_crud() {
    let (store, _dir) = make_store();
    store
        .create_channel("eng", None, ChannelType::Channel)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();

    let tasks = store
        .create_tasks("eng", "bot1", &["Fix bug", "Add feature"])
        .unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].task_number, 1);
    assert_eq!(tasks[1].task_number, 2);

    let listed = store.list_tasks("eng", None).unwrap();
    assert_eq!(listed.len(), 2);
}

#[test]
fn test_task_claim_and_status() {
    let (store, _dir) = make_store();
    store
        .create_channel("eng", None, ChannelType::Channel)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "o3", &[])
        .unwrap();
    store.create_tasks("eng", "bot1", &["Task A"]).unwrap();

    let results = store.claim_tasks("eng", "bot1", &[1]).unwrap();
    assert!(results[0].success);

    let results = store.claim_tasks("eng", "bot2", &[1]).unwrap();
    assert!(!results[0].success);

    store
        .update_task_status("eng", 1, "bot1", TaskStatus::InReview)
        .unwrap();
    let tasks = store.list_tasks("eng", Some(TaskStatus::InReview)).unwrap();
    assert_eq!(tasks.len(), 1);
}

#[test]
fn test_resolve_target() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();

    let (ch_id, thread_parent) = store.resolve_target("#general", "bot1").unwrap();
    assert!(!ch_id.is_empty());
    assert!(thread_parent.is_none());

    let (dm_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    assert!(!dm_id.is_empty());
}

#[test]
fn test_list_channels_excludes_dm() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    // Create a DM channel via resolve_target
    store.resolve_target("dm:@alice", "bot1").unwrap();

    let channels = store.list_channels().unwrap();
    assert_eq!(
        channels.len(),
        1,
        "list_channels must not return DM channels"
    );
    assert_eq!(channels[0].name, "general");
}

#[test]
fn test_dm_only_has_two_members() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .create_agent_record("bot2", "Bot 2", None, "claude", "sonnet", &[])
        .unwrap();

    // Create DM between alice and bot1
    let (dm_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();

    // Simulate old bug: manually add bot2 as a spurious member
    store
        .join_channel("dm-alice-bot1", "bot2", SenderType::Agent)
        .unwrap();

    let members = store.get_channel_members(&dm_id).unwrap();
    assert_eq!(members.len(), 3, "precondition: spurious member added");

    // Re-open store to trigger the migration
    let db_path = _dir.path().join("test.db");
    let store2 = Store::open(db_path.to_str().unwrap()).unwrap();
    let members2 = store2.get_channel_members(&dm_id).unwrap();
    assert_eq!(
        members2.len(),
        2,
        "migration must remove spurious DM members"
    );
    let names: Vec<_> = members2.iter().map(|m| m.member_name.as_str()).collect();
    assert!(names.contains(&"alice"), "alice must remain");
    assert!(names.contains(&"bot1"), "bot1 must remain");
}

#[test]
fn test_dm_channels() {
    let (store, _dir) = make_store();
    store.add_human("alice").unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();

    let (ch_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    let (ch_id2, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    assert_eq!(ch_id, ch_id2);
}

#[test]
fn test_unrelated_agents_do_not_receive_thread_messages() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4", &[])
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();

    let parent_id = store
        .send_message(
            "general",
            None,
            "alice",
            SenderType::Human,
            "human parent",
            &[],
        )
        .unwrap();

    let initial = store.get_messages_for_agent("bot2", true).unwrap();
    assert_eq!(
        initial.len(),
        1,
        "precondition: bot2 sees the parent message"
    );

    store
        .send_message(
            "general",
            Some(&parent_id),
            "bot1",
            SenderType::Agent,
            "bot1 thread reply",
            &[],
        )
        .unwrap();

    let bot2_messages = store.get_messages_for_agent("bot2", false).unwrap();
    assert!(
        bot2_messages.is_empty(),
        "unrelated agents should not receive thread replies just because they share the parent channel"
    );

    let bot1_messages = store.get_messages_for_agent("bot1", false).unwrap();
    assert!(
        bot1_messages
            .iter()
            .all(|message| message.content != "bot1 thread reply"),
        "the replying agent should not get its own thread reply back as unread"
    );
}

#[test]
fn test_update_channel_preserves_id_and_metadata() {
    let (store, dir) = make_store();
    let channel_id = store
        .create_channel("general", Some("General channel"), ChannelType::Channel)
        .unwrap();

    store
        .update_channel(&channel_id, "engineering", Some("Engineering"))
        .unwrap();

    let renamed = store.find_channel_by_name("engineering").unwrap().unwrap();
    assert_eq!(renamed.id, channel_id);
    assert_eq!(renamed.description.as_deref(), Some("Engineering"));
    assert!(
        store.find_channel_by_name("general").unwrap().is_none(),
        "old name should no longer resolve after rename"
    );

    let conn = Connection::open(dir.path().join("test.db")).unwrap();
    let raw_id: String = conn
        .query_row(
            "SELECT id FROM channels WHERE name = 'engineering'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw_id, channel_id);
}

#[test]
fn test_archive_channel_hides_it_from_active_listings() {
    let (store, _dir) = make_store();
    let channel_id = store
        .create_channel("general", Some("General channel"), ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    store.archive_channel(&channel_id).unwrap();

    assert!(
        store.find_channel_by_id(&channel_id).unwrap().is_some(),
        "archive should preserve the underlying channel row"
    );
    assert!(
        store.list_channels().unwrap().is_empty(),
        "archived channels must be hidden from active channel listings"
    );
    assert!(
        chorus::server::build_server_info(&store, "alice")
            .unwrap()
            .channels
            .is_empty(),
        "archived channels must not appear in server info"
    );
}

#[test]
fn test_ensure_builtin_channels_migrates_general_to_all_system_channel() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", Some("General channel"), ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let original = store.find_channel_by_name("general").unwrap().unwrap();

    store.ensure_builtin_channels("alice").unwrap();

    assert!(
        store.find_channel_by_name("general").unwrap().is_none(),
        "startup migration should rename #general to #all"
    );

    let all = store.find_channel_by_name("all").unwrap().unwrap();
    assert_eq!(all.id, original.id);
    assert_eq!(all.channel_type, ChannelType::System);
    assert_eq!(
        all.description.as_deref(),
        Some("All members"),
        "the migrated default channel should use the new built-in description"
    );
    assert!(
        store.is_member("all", "alice").unwrap(),
        "existing memberships should survive the rename"
    );

    let server_info = chorus::server::build_server_info(&store, "alice").unwrap();
    assert!(
        server_info
            .channels
            .iter()
            .all(|channel| channel.name != "all"),
        "#all should no longer appear in the editable channel list"
    );
    let system_all = server_info
        .system_channels
        .iter()
        .find(|channel| channel.name == "all")
        .expect("#all should be listed as a system channel");
    assert!(
        !system_all.read_only,
        "#all must remain writable even though it is classified as a system channel"
    );
}

#[test]
fn test_ensure_builtin_channels_backfills_all_existing_humans_and_agents() {
    let (store, _dir) = make_store();
    store.add_human("alice").unwrap();
    store.add_human("zoe").unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4-mini", &[])
        .unwrap();

    store.ensure_builtin_channels("alice").unwrap();

    let all = store.find_channel_by_name("all").unwrap().unwrap();
    let members = store.get_channel_members(&all.id).unwrap();
    let names: Vec<_> = members
        .iter()
        .map(|member| member.member_name.as_str())
        .collect();
    assert!(names.contains(&"alice"));
    assert!(names.contains(&"zoe"));
    assert!(names.contains(&"bot1"));
    assert!(names.contains(&"bot2"));
}

#[test]
fn test_new_humans_and_agents_auto_join_all_when_it_exists() {
    let (store, _dir) = make_store();
    store.add_human("alice").unwrap();
    store.ensure_builtin_channels("alice").unwrap();

    store.add_human("zoe").unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();

    assert!(store.is_member("all", "alice").unwrap());
    assert!(store.is_member("all", "zoe").unwrap());
    assert!(store.is_member("all", "bot1").unwrap());
}

#[test]
fn test_delete_channel_removes_messages_tasks_and_memberships() {
    let (store, dir) = make_store();
    let channel_id = store
        .create_channel("eng", Some("Engineering"), ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("eng", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("eng", "bot1", SenderType::Agent)
        .unwrap();

    let parent_id = store
        .send_message("eng", None, "alice", SenderType::Human, "hello", &[])
        .unwrap();
    store
        .send_message(
            "eng",
            Some(&parent_id),
            "bot1",
            SenderType::Agent,
            "thread reply",
            &[],
        )
        .unwrap();
    store.create_tasks("eng", "bot1", &["ship it"]).unwrap();

    store.delete_channel(&channel_id).unwrap();

    assert!(
        store.find_channel_by_id(&channel_id).unwrap().is_none(),
        "channel row should be removed after hard delete"
    );
    let conn = Connection::open(dir.path().join("test.db")).unwrap();
    let membership_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1",
            [&channel_id],
            |row| row.get(0),
        )
        .unwrap();
    let message_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE channel_id = ?1",
            [&channel_id],
            |row| row.get(0),
        )
        .unwrap();
    let task_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE channel_id = ?1",
            [&channel_id],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(membership_count, 0, "channel memberships should cascade");
    assert_eq!(message_count, 0, "channel messages should cascade");
    assert_eq!(task_count, 0, "channel tasks should cascade");
}

#[test]
fn test_record_swarm_signal_ignores_non_quorum_agent() {
    let store = Store::open(":memory:").unwrap();
    // Create a team channel so we have a valid channel_id for the trigger message.
    store
        .create_channel("qa-swarm", None, ChannelType::Team)
        .unwrap();
    // Insert a messages row directly so we have a trigger_message_id without needing
    // the full send_message flow (which requires channel membership setup).
    let trigger_id = {
        let conn = store.conn_for_test();
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO messages (id, channel_id, sender_name, sender_type, content, seq) \
             SELECT ?1, c.id, 'human', 'human', 'task', 1 FROM channels c WHERE c.name = 'qa-swarm'",
            rusqlite::params![id],
        )
        .unwrap();
        id
    };
    // Create team with alice and bob as members.
    let team_id = store
        .create_team("qa-swarm", "QA Swarm", "swarm", None)
        .unwrap();
    store
        .add_team_member(&team_id, "alice", "agent", "uuid-alice", "member")
        .unwrap();
    store
        .add_team_member(&team_id, "bob", "agent", "uuid-bob", "member")
        .unwrap();
    // Snapshot quorum — only alice and bob are captured.
    store.snapshot_swarm_quorum(&team_id, &trigger_id).unwrap();
    // Charlie is NOT in the quorum; their signal must be silently discarded.
    let resolved = store
        .record_swarm_signal(&team_id, "charlie", "READY: I'll do the thing")
        .unwrap();
    assert!(
        !resolved,
        "non-quorum agent should not contribute to consensus"
    );
    // The quorum must still be unresolved after charlie's ignored signal.
    let conn = store.conn_for_test();
    let resolved_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM team_task_quorum WHERE trigger_message_id = ?1 AND resolved_at IS NOT NULL",
            rusqlite::params![trigger_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        resolved_count, 0,
        "quorum should still be unresolved after non-quorum signal"
    );
}
