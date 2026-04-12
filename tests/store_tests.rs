use chorus::store::agents::AgentEnvVar;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, SenderType};
use chorus::store::tasks::TaskStatus;
use chorus::store::{AgentRecordUpsert, Store};
use rusqlite::{params, Connection};
use tempfile::tempdir;

#[test]
fn test_team_tables_exist() {
    let store = Store::open(":memory:").unwrap();
    let conn = store.conn_for_test();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('teams','team_members')",
        [],
        |r| r.get::<_, i64>(0),
    ).unwrap();
    assert_eq!(count, 2);
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
        .create_team_member(&team_id, "alice", "agent", "agent-uuid-1", "operator")
        .unwrap();
    store
        .create_team_member(&team_id, "bob", "human", "bob", "observer")
        .unwrap();
    let members = store.get_team_members(&team_id).unwrap();
    assert_eq!(members.len(), 2);
}

#[test]
fn test_list_teams_for_agent() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store
        .create_team_member(&team_id, "alice", "agent", "agent-uuid-1", "operator")
        .unwrap();
    let teams = store.get_teams_by_agent_name("alice").unwrap();
    assert_eq!(teams.len(), 1);
    assert_eq!(teams[0].team_name, "eng-team");
}

#[test]
fn test_delete_team_cascades() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store
        .create_team_member(&team_id, "alice", "agent", "uuid-1", "operator")
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

/// Channel archive, team creation, and agent record update as performed by shell/API layers.
/// Lives in store tests (not `server_tests`) so we do not build the HTTP router: constructing
/// `ServeDir::new("ui/dist")` can block for a long time when that tree is huge or on a slow volume.
#[test]
fn test_shell_style_workspace_mutations_persist() {
    let store = Store::open(":memory:").unwrap();
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();

    store
        .create_channel(
            "ops-room",
            Some("Shell API mutation coverage"),
            ChannelType::Channel,
        )
        .unwrap();
    store
        .join_channel("ops-room", "alice", SenderType::Human)
        .unwrap();
    let channel_id = store.get_channel_by_name("ops-room").unwrap().unwrap().id;
    store.archive_channel(&channel_id).unwrap();

    let team_id = store
        .create_team("ops-team", "Ops Team", "leader_operators", Some("bot1"))
        .unwrap();
    store
        .create_channel("ops-team", None, ChannelType::Team)
        .unwrap();
    store
        .create_team_member(&team_id, "bot1", "agent", &bot1.id, "operator")
        .unwrap();
    store
        .join_channel("ops-team", "bot1", SenderType::Agent)
        .unwrap();

    store
        .update_agent_record_with_reasoning(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Ops Bot",
            description: Some("Updated from shell mutation test"),
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();

    let archived: i64 = {
        let conn = store.conn_for_test();
        conn.query_row(
            "SELECT archived FROM channels WHERE name = ?1",
            params!["ops-room"],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(archived, 1);
    assert!(store.get_team("ops-team").unwrap().is_some());
    let updated = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(updated.display_name, "Ops Bot");
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
    let channels = store.get_channels().unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].name, "general");
}

#[test]
fn test_send_and_receive_messages() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let msg_id = store
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
    assert!(!msg_id.is_empty());

    let msgs = store.get_messages_for_agent("bob", false).unwrap();
    assert!(msgs.is_empty());

    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    let _msg_id2 = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "hello bot",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let msgs = store.get_messages_for_agent("bot1", false).unwrap();
    assert_eq!(msgs.len(), 2);
}

#[test]
fn test_agent_does_not_receive_its_own_sent_message() {
    let (store, _dir) = make_store();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store.create_human("alice").unwrap();
    let (dm_channel_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    let dm_channel = store.get_channel_by_id(&dm_channel_id).unwrap().unwrap();

    store
        .create_message(CreateMessage {
            channel_name: &dm_channel.name,
            thread_parent_id: None,
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "hello alice",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
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
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    for i in 0..10 {
        store
            .create_message(CreateMessage {
                channel_name: "general",
                thread_parent_id: None,
                sender_name: "alice",
                sender_type: SenderType::Human,
                content: &format!("msg {i}"),
                attachment_ids: &[],
                suppress_event: false,
                run_id: None,
            })
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
fn test_history_snapshot_returns_messages_and_read_cursor() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "one",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "two",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let snapshot = store
        .get_history_snapshot("general", "alice", None, 10, None, None)
        .unwrap();

    assert_eq!(snapshot.messages.len(), 2);
    assert!(!snapshot.has_more);
    assert_eq!(snapshot.last_read_seq, 2);
    assert_eq!(snapshot.messages[0].content, "one");
    assert_eq!(snapshot.messages[1].content, "two");
}

#[test]
fn test_inbox_conversation_state_view_projects_last_read_and_unread_count() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    let first_top_level = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "one",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&first_top_level),
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "thread reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let second_top_level = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "two",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let state_before = store
        .get_inbox_conversation_state("general", "bot1")
        .unwrap()
        .unwrap();
    assert_eq!(state_before.conversation_name, "general");
    assert_eq!(state_before.member_name, "bot1");
    assert_eq!(state_before.last_read_seq, 0);
    assert_eq!(state_before.last_read_message_id, None);
    assert_eq!(state_before.unread_count, 2);

    let conn = store.conn_for_test();
    let row = conn
        .query_row(
            "SELECT conversation_name, member_name, last_read_seq, last_read_message_id, unread_count
             FROM inbox_conversation_state_view
             WHERE conversation_name = 'general' AND member_name = 'bot1'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0, "general");
    assert_eq!(row.1, "bot1");
    assert_eq!(row.2, 0);
    assert_eq!(row.3, None);
    assert_eq!(row.4, 2);
    drop(conn);

    let unread = store.get_messages_for_agent("bot1", true).unwrap();
    assert_eq!(unread.len(), 2);

    let state_after = store
        .get_inbox_conversation_state("general", "bot1")
        .unwrap()
        .unwrap();
    assert_eq!(state_after.last_read_seq, 3);
    assert_eq!(
        state_after.last_read_message_id.as_deref(),
        Some(second_top_level.as_str())
    );
    assert_eq!(state_after.unread_count, 0);

    let conn = store.conn_for_test();
    let legacy_last_read_seq: i64 = conn
        .query_row(
            "SELECT last_read_seq FROM channel_members
             WHERE channel_id = ?1 AND member_name = 'bot1'",
            params![state_after.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        legacy_last_read_seq, 0,
        "legacy channel_members.last_read_seq should no longer own inbox read state"
    );
}

#[test]
fn test_history_snapshot_and_unread_summary_use_inbox_projection() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "one",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "two",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let unread_before = store.get_unread_summary("bot1").unwrap();
    assert_eq!(unread_before.get("general"), Some(&2));

    let snapshot_before = store
        .get_history_snapshot("general", "bot1", None, 10, None, None)
        .unwrap();
    assert_eq!(snapshot_before.last_read_seq, 0);

    store.get_messages_for_agent("bot1", true).unwrap();

    let unread_after = store.get_unread_summary("bot1").unwrap();
    assert_eq!(unread_after.get("general"), None);

    let snapshot_after = store
        .get_history_snapshot("general", "bot1", None, 10, None, None)
        .unwrap();
    assert_eq!(snapshot_after.last_read_seq, 2);
    assert_eq!(store.get_last_read_seq("general", "bot1").unwrap(), 2);
}

#[test]
fn test_explicit_thread_read_cursor_persists_separately_from_conversation_cursor() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let reply_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let thread_before = store
        .get_thread_notification_state("general", &parent_id, "alice")
        .unwrap()
        .unwrap();
    assert_eq!(thread_before.last_read_seq, 0);
    assert_eq!(thread_before.unread_count, 1);

    store
        .set_history_read_cursor("general", "alice", SenderType::Human, Some(&parent_id), 2)
        .unwrap();

    let thread_after = store
        .get_thread_notification_state("general", &parent_id, "alice")
        .unwrap()
        .unwrap();
    assert_eq!(thread_after.last_read_seq, 2);
    assert_eq!(thread_after.unread_count, 0);
    assert_eq!(
        thread_after.last_reply_message_id.as_deref(),
        Some(reply_id.as_str())
    );

    let conversation_snapshot = store
        .get_history_snapshot("general", "alice", None, 10, None, None)
        .unwrap();
    assert_eq!(conversation_snapshot.last_read_seq, 0);

    let thread_snapshot = store
        .get_history_snapshot("general", "alice", Some(&parent_id), 10, None, None)
        .unwrap();
    assert_eq!(thread_snapshot.last_read_seq, 2);
}

#[test]
fn test_history_read_cursor_rejects_seq_above_max() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "a",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "b",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let err = store
        .set_history_read_cursor("general", "alice", SenderType::Human, None, 999)
        .unwrap_err();
    assert!(
        err.to_string().contains("greater than latest message seq"),
        "unexpected error: {err}"
    );
    // Sending advances the sender's read cursor to the latest seq (2).
    assert_eq!(store.get_last_read_seq("general", "alice").unwrap(), 2);
}

#[test]
fn test_history_read_cursor_rejects_negative_seq() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "a",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let err = store
        .set_history_read_cursor("general", "alice", SenderType::Human, None, -1)
        .unwrap_err();
    assert!(
        err.to_string().contains("non-negative"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_history_read_cursor_heals_orphan_above_max_seq() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "a",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let channel = store.get_channel_by_name("general").unwrap().unwrap();
    {
        let conn = store.conn_for_test();
        conn.execute(
            "INSERT INTO inbox_read_state (
                conversation_id, member_name, member_type, last_read_seq, last_read_message_id, updated_at
             ) VALUES (?1, 'alice', 'human', 50, NULL, datetime('now'))
             ON CONFLICT(conversation_id, member_name) DO UPDATE SET last_read_seq = excluded.last_read_seq",
            params![channel.id],
        )
        .unwrap();
    }

    store
        .set_history_read_cursor("general", "alice", SenderType::Human, None, 1)
        .unwrap();
    assert_eq!(store.get_last_read_seq("general", "alice").unwrap(), 1);
}

#[test]
fn test_conversation_messages_view_projects_message_rows() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let message_id = store
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
    let channel = store.get_channel_by_name("general").unwrap().unwrap();

    let conn = store.conn_for_test();
    let row = conn
        .query_row(
            "SELECT message_id, conversation_id, conversation_name, conversation_type,
                    thread_parent_id, sender_name, sender_type, sender_deleted, content, seq
             FROM conversation_messages_view
             WHERE message_id = ?1",
            params![message_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(row.0, message_id);
    assert_eq!(row.1, channel.id);
    assert_eq!(row.2, "general");
    assert_eq!(row.3, "channel");
    assert_eq!(row.4, None);
    assert_eq!(row.5, "alice");
    assert_eq!(row.6, "human");
    assert_eq!(row.7, 0);
    assert_eq!(row.8, "hello");
    assert_eq!(row.9, 1);
}

#[test]
fn test_conversation_message_view_matches_history_projection() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let projection = store
        .get_conversation_message_view(&parent_id)
        .unwrap()
        .unwrap();
    let snapshot = store
        .get_history_snapshot("general", "alice", None, 10, None, None)
        .unwrap();

    assert_eq!(projection.message_id, parent_id);
    assert_eq!(projection.conversation_name, "general");
    assert_eq!(projection.conversation_type, "channel");
    assert_eq!(projection.thread_parent_id, None);
    assert_eq!(projection.sender_name, "alice");
    assert_eq!(projection.sender_type, "human");
    assert!(!projection.sender_deleted);
    assert_eq!(projection.content, "parent");
    assert_eq!(projection.reply_count, Some(1));
    assert_eq!(projection.attachments.len(), 0);

    let history_message = &snapshot.messages[0];
    assert_eq!(history_message.id, projection.message_id);
    assert_eq!(history_message.content, projection.content);
    assert_eq!(history_message.sender_name, projection.sender_name);
    assert_eq!(history_message.sender_type, projection.sender_type);
    assert_eq!(history_message.created_at, projection.created_at);
    assert_eq!(history_message.sender_deleted, projection.sender_deleted);
    assert_eq!(history_message.reply_count, projection.reply_count);
    assert!(history_message.attachments.is_none());
}

#[test]
fn test_thread_summaries_view_projects_thread_metadata() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "reply one",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let last_reply_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "reply two",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let channel = store.get_channel_by_name("general").unwrap().unwrap();

    let conn = store.conn_for_test();
    let row = conn
        .query_row(
            "SELECT conversation_id, parent_message_id, reply_count,
                    last_reply_message_id, last_reply_at, participant_count
             FROM thread_summaries_view
             WHERE conversation_id = ?1 AND parent_message_id = ?2",
            params![channel.id.as_str(), parent_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(row.0, channel.id);
    assert_eq!(row.1, parent_id);
    assert_eq!(row.2, 2);
    assert_eq!(row.3.as_deref(), Some(last_reply_id.as_str()));
    assert!(row.4.is_some());
    assert_eq!(row.5, 2);
}

#[test]
fn test_thread_summary_projection_matches_history() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let last_reply_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let summary = store.get_thread_summary_view(&parent_id).unwrap().unwrap();
    let snapshot = store
        .get_history_snapshot("general", "alice", None, 10, None, None)
        .unwrap();

    assert_eq!(summary.parent_message_id, parent_id);
    assert_eq!(summary.reply_count, 1);
    assert_eq!(
        summary.last_reply_message_id.as_deref(),
        Some(last_reply_id.as_str())
    );
    assert!(summary.last_reply_at.is_some());
    assert_eq!(summary.participant_count, 1);
    assert_eq!(snapshot.messages[0].reply_count, Some(summary.reply_count));
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
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &env_vars,
            })
        .unwrap();

    let agent = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(agent.env_vars, env_vars);
}

#[test]
fn test_agent_reasoning_effort_persists_in_agent_record() {
    let (store, _dir) = make_store();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
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
            system_prompt: None,
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
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    store.mark_agent_messages_deleted("bot1").unwrap();
    let (history, _) = store.get_history("general", None, 10, None, None).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].sender_deleted);
}

#[test]
fn test_create_message_persists_top_level() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let message_id = store
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
    let (history, _) = store.get_history("general", None, 10, None, None).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, message_id);
    assert_eq!(history[0].content, "hello");
    assert_eq!(history[0].sender_name, "alice");
}

#[test]
fn test_thread_reply_updates_inbox_read_models() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let reply_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let (thread_history, _) = store
        .get_history("general", Some(&parent_id), 10, None, None)
        .unwrap();
    assert_eq!(thread_history.len(), 1);
    assert_eq!(thread_history[0].id, reply_id);

    let alice_conversation_state = store
        .get_inbox_conversation_state("general", "alice")
        .unwrap()
        .unwrap();
    assert_eq!(alice_conversation_state.last_read_seq, 1);
    // unread_count only counts top-level messages, not thread replies
    assert_eq!(alice_conversation_state.unread_count, 0);
    // thread_unread_count counts thread replies
    assert_eq!(alice_conversation_state.thread_unread_count, 1);

    let alice_thread_state = store
        .get_thread_notification_state("general", &parent_id, "alice")
        .unwrap()
        .unwrap();
    assert_eq!(alice_thread_state.latest_seq, 2);
    assert_eq!(alice_thread_state.last_read_seq, 0);
    assert_eq!(alice_thread_state.unread_count, 1);
}

#[test]
fn test_channel_thread_inbox_returns_rows_ordered_by_latest_reply_desc() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    let oldest_unread_parent = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "oldest unread parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&oldest_unread_parent),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "oldest unread reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let newest_read_parent = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "newest read parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let _newest_read_reply = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&newest_read_parent),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "newest read reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .set_history_read_cursor(
            "general",
            "alice",
            SenderType::Human,
            Some(&newest_read_parent),
            4,
        )
        .unwrap();

    let newest_unread_parent = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "newest unread parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let _newest_unread_reply = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&newest_unread_parent),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "newest unread reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let inbox = store.get_channel_thread_inbox("general", "alice").unwrap();

    assert_eq!(inbox.unread_count, 2);
    assert_eq!(inbox.threads.len(), 3);

    assert_eq!(inbox.threads[0].thread_parent_id, newest_unread_parent);
    assert_eq!(inbox.threads[0].unread_count, 1);
    assert_eq!(inbox.threads[0].reply_count, 1);
    assert_eq!(
        inbox.threads[0].last_reply_message_id.as_deref(),
        Some(_newest_unread_reply.as_str())
    );
    assert_eq!(inbox.threads[0].parent_content, "newest unread parent");

    assert_eq!(inbox.threads[1].thread_parent_id, newest_read_parent);
    assert_eq!(inbox.threads[1].unread_count, 0);
    assert_eq!(inbox.threads[1].parent_content, "newest read parent");

    assert_eq!(inbox.threads[2].thread_parent_id, oldest_unread_parent);
    assert_eq!(inbox.threads[2].unread_count, 1);
    assert_eq!(inbox.threads[2].parent_content, "oldest unread parent");
}

#[test]
fn test_unread_excludes_own_messages_for_sender() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "from bot a",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "from bot b",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let channel_id = store
        .get_channel_by_name("general")
        .unwrap()
        .expect("general exists")
        .id;
    let conn = store.conn_for_test();
    conn.execute(
        "UPDATE inbox_read_state SET last_read_seq = 0, last_read_message_id = NULL
         WHERE conversation_id = ?1",
        params![channel_id],
    )
    .unwrap();
    drop(conn);

    let bot_state = store
        .get_inbox_conversation_state("general", "bot1")
        .unwrap()
        .unwrap();
    assert_eq!(bot_state.unread_count, 0);

    let alice_state = store
        .get_inbox_conversation_state("general", "alice")
        .unwrap()
        .unwrap();
    assert_eq!(alice_state.unread_count, 2);

    let parent = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let conn = store.conn_for_test();
    conn.execute(
        "UPDATE inbox_thread_read_state SET last_read_seq = 0, last_read_message_id = NULL
         WHERE conversation_id = ?1 AND thread_parent_id = ?2 AND member_name = 'bot1'",
        params![channel_id, parent],
    )
    .unwrap();
    drop(conn);

    let bot_thread = store
        .get_thread_notification_state("general", &parent, "bot1")
        .unwrap()
        .unwrap();
    assert_eq!(bot_thread.unread_count, 0);
}

#[test]
fn test_tasks_crud() {
    let (store, _dir) = make_store();
    store
        .create_channel("eng", None, ChannelType::Channel)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();

    let tasks = store
        .create_tasks("eng", "bot1", &["Fix bug", "Add feature"])
        .unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].task_number, 1);
    assert_eq!(tasks[1].task_number, 2);

    let listed = store.get_tasks("eng", None).unwrap();
    assert_eq!(listed.len(), 2);
}

#[test]
fn test_task_claim_and_status() {
    let (store, _dir) = make_store();
    store
        .create_channel("eng", None, ChannelType::Channel)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot2",
                display_name: "Bot 2",
                description: None,
                system_prompt: None,
                runtime: "codex",
                model: "o3",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store.create_tasks("eng", "bot1", &["Task A"]).unwrap();

    let results = store.update_tasks_claim("eng", "bot1", &[1]).unwrap();
    assert!(results[0].success);

    let results = store.update_tasks_claim("eng", "bot2", &[1]).unwrap();
    assert!(!results[0].success);

    store
        .update_task_status("eng", 1, "bot1", TaskStatus::InReview)
        .unwrap();
    let tasks = store.get_tasks("eng", Some(TaskStatus::InReview)).unwrap();
    assert_eq!(tasks.len(), 1);
}

#[test]
fn test_resolve_target() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
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
    store.create_human("alice").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    // Create a DM channel via resolve_target
    store.resolve_target("dm:@alice", "bot1").unwrap();

    let channels = store.get_channels().unwrap();
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
    store.create_human("alice").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot2",
                display_name: "Bot 2",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
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
    store.create_human("alice").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
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
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot2",
                display_name: "Bot 2",
                description: None,
                system_prompt: None,
                runtime: "codex",
                model: "gpt-5.4",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();

    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "human parent",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let initial = store.get_messages_for_agent("bot2", true).unwrap();
    assert_eq!(
        initial.len(),
        1,
        "precondition: bot2 sees the parent message"
    );

    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "bot1 thread reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
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

    let renamed = store.get_channel_by_name("engineering").unwrap().unwrap();
    assert_eq!(renamed.id, channel_id);
    assert_eq!(renamed.description.as_deref(), Some("Engineering"));
    assert!(
        store.get_channel_by_name("general").unwrap().is_none(),
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
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    store.archive_channel(&channel_id).unwrap();

    assert!(
        store.get_channel_by_id(&channel_id).unwrap().is_some(),
        "archive should preserve the underlying channel row"
    );
    assert!(
        store.get_channels().unwrap().is_empty(),
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
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let original = store.get_channel_by_name("general").unwrap().unwrap();

    store.ensure_builtin_channels("alice").unwrap();

    assert!(
        store.get_channel_by_name("general").unwrap().is_none(),
        "startup migration should rename #general to #all"
    );

    let all = store.get_channel_by_name("all").unwrap().unwrap();
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
    store.create_human("alice").unwrap();
    store.create_human("zoe").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot2",
                display_name: "Bot 2",
                description: None,
                system_prompt: None,
                runtime: "codex",
                model: "gpt-5.4-mini",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();

    store.ensure_builtin_channels("alice").unwrap();

    let all = store.get_channel_by_name("all").unwrap().unwrap();
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
fn test_ensure_builtin_channels_only_exposes_all_system_channel() {
    let (store, _dir) = make_store();
    store.create_human("alice").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();

    store.ensure_builtin_channels("alice").unwrap();

    assert!(
        store
            .get_channel_by_name("shared-memory")
            .unwrap()
            .is_none(),
        "shared-memory should no longer be created as a built-in system channel"
    );

    let server_info = chorus::server::build_server_info(&store, "bot1").unwrap();
    let system_names: Vec<_> = server_info
        .system_channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect();
    assert_eq!(system_names, vec!["all"]);
}

#[test]
fn test_store_open_removes_legacy_shared_memory_system_channel() {
    let (store, dir) = make_store();
    store.create_human("alice").unwrap();
    store.ensure_builtin_channels("alice").unwrap();
    store
        .ensure_system_channel("shared-memory", "Agent group memory")
        .unwrap();
    drop(store);

    let db_path = dir.path().join("test.db");
    let reopened = Store::open(db_path.to_str().unwrap()).unwrap();

    assert!(
        reopened
            .get_channel_by_name("shared-memory")
            .unwrap()
            .is_none(),
        "legacy shared-memory system channel should be removed during startup migration"
    );

    let server_info = chorus::server::build_server_info(&reopened, "alice").unwrap();
    let system_names: Vec<_> = server_info
        .system_channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect();
    assert_eq!(system_names, vec!["all"]);
}

#[test]
fn test_new_humans_and_agents_auto_join_all_when_it_exists() {
    let (store, _dir) = make_store();
    store.create_human("alice").unwrap();
    store.ensure_builtin_channels("alice").unwrap();

    store.create_human("zoe").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
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
    store.create_human("alice").unwrap();
    store
        .join_channel("eng", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("eng", "bot1", SenderType::Agent)
        .unwrap();

    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "eng",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "eng",
            thread_parent_id: Some(&parent_id),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "thread reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store.create_tasks("eng", "bot1", &["ship it"]).unwrap();

    store.delete_channel(&channel_id).unwrap();

    assert!(
        store.get_channel_by_id(&channel_id).unwrap().is_none(),
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

/// Verifies that thread_unread_count (yellow dot indicator) becomes zero after
/// reading all messages in both channel and all threads.
/// This ensures the yellow dot correctly disappears when user has no unread content.
#[test]
fn test_thread_unread_count_clears_after_reading_all_messages() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    // Setup: Create channel message + thread reply (simulating unread state)
    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "channel message",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let reply_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "thread reply",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    // Verify initial state: unread exists
    let state_before = store
        .get_inbox_conversation_state("general", "alice")
        .unwrap()
        .unwrap();
    assert_eq!(
        state_before.unread_count, 1,
        "channel message should be unread"
    );
    assert_eq!(
        state_before.thread_unread_count, 1,
        "thread reply should be unread (yellow dot should show)"
    );

    // Step 1: Read the channel message (seq=1)
    store
        .set_history_read_cursor("general", "alice", SenderType::Human, None, 1)
        .unwrap();

    let state_after_channel_read = store
        .get_inbox_conversation_state("general", "alice")
        .unwrap()
        .unwrap();
    assert_eq!(
        state_after_channel_read.unread_count, 0,
        "channel message should now be read"
    );
    assert_eq!(
        state_after_channel_read.thread_unread_count, 1,
        "thread unread should remain (yellow dot still shows)"
    );

    // Step 2: Read the thread reply (seq=2)
    store
        .set_history_read_cursor("general", "alice", SenderType::Human, Some(&parent_id), 2)
        .unwrap();

    let state_after_thread_read = store
        .get_inbox_conversation_state("general", "alice")
        .unwrap()
        .unwrap();
    assert_eq!(
        state_after_thread_read.unread_count, 0,
        "channel message should still be read"
    );
    assert_eq!(
        state_after_thread_read.thread_unread_count, 0,
        "thread unread should be zero (yellow dot should disappear)"
    );

    // Verify thread-specific state also shows zero unread
    let thread_state = store
        .get_thread_notification_state("general", &parent_id, "alice")
        .unwrap()
        .unwrap();
    assert_eq!(thread_state.unread_count, 0);
    assert_eq!(thread_state.last_read_seq, 2);
    assert_eq!(
        thread_state.last_reply_message_id.as_deref(),
        Some(reply_id.as_str())
    );
}

// ── System message tests ──

#[test]
fn test_sender_type_system_round_trip() {
    // Pure enum round-trip — no store needed.
    assert_eq!(SenderType::System.as_str(), "system");
    assert_eq!(
        SenderType::from_sender_type_str("system"),
        SenderType::System
    );
    // Existing variants still work.
    assert_eq!(SenderType::Human.as_str(), "human");
    assert_eq!(SenderType::Agent.as_str(), "agent");
    assert_eq!(SenderType::from_sender_type_str("agent"), SenderType::Agent);
    // Unknown values still default to Human (legacy safety).
    assert_eq!(SenderType::from_sender_type_str("??"), SenderType::Human);
}

#[test]
fn test_create_system_message_writes_system_sender_type() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    let msg_id = store
        .create_system_message(&channel_id, "Team assembled: Alpha, Beta.")
        .unwrap();
    assert!(!msg_id.is_empty());

    let (history, _) = store.get_history("general", None, 10, None, None).unwrap();
    let sys_msg = history
        .iter()
        .find(|m| m.id == msg_id)
        .expect("system message should appear in history");
    assert_eq!(sys_msg.sender_name, "system");
    assert_eq!(
        sys_msg.sender_type, "system",
        "sender_type should be 'system', not 'human'"
    );
    assert_eq!(sys_msg.content, "Team assembled: Alpha, Beta.");
}

#[test]
fn test_channel_unread_count_excludes_system_messages() {
    // The primary regression guard for the inbox_conversation_state_view change.
    // A server-authored system message must not bump the channel unread badge.
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store.create_human("bob").unwrap();
    store
        .join_channel("general", "bob", SenderType::Human)
        .unwrap();

    // A regular message from bob — alice should see 1 unread.
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "bob",
            sender_type: SenderType::Human,
            content: "hey",
            attachment_ids: &[],
            run_id: None,
            suppress_event: false,
        })
        .unwrap();

    let unread_before = store.get_unread_summary("alice").unwrap();
    assert_eq!(
        unread_before.get("general"),
        Some(&1),
        "baseline: one human message counts as unread"
    );

    // A system message — must NOT bump alice's unread.
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    store
        .create_system_message(&channel_id, "Team assembled.")
        .unwrap();

    let unread_after = store.get_unread_summary("alice").unwrap();
    assert_eq!(
        unread_after.get("general"),
        Some(&1),
        "system message must not increment channel unread count"
    );
}

#[test]
fn test_thread_view_excludes_system_replies() {
    // Defensive test: system replies don't exist through the public API today,
    // but the SQL view's thread-level subquery was updated alongside the
    // channel-level one. Insert a system thread reply directly to exercise
    // the exact SQL path that was changed.
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store.create_human("bob").unwrap();
    store
        .join_channel("general", "bob", SenderType::Human)
        .unwrap();

    // Alice posts a top-level message. Bob will use this as a thread parent.
    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "parent",
            attachment_ids: &[],
            run_id: None,
            suppress_event: false,
        })
        .unwrap();

    // Bob posts one human reply — alice should see 1 thread unread.
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "bob",
            sender_type: SenderType::Human,
            content: "bob reply",
            attachment_ids: &[],
            run_id: None,
            suppress_event: false,
        })
        .unwrap();

    // Directly insert a system reply bypassing the public API, to prove the
    // view's `reply.sender_type != 'system'` filter is doing its job.
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    {
        let conn = store.conn_for_test();
        let max_seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE channel_id = ?1",
                params![channel_id],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO messages (id, channel_id, thread_parent_id, sender_name, sender_type, content, seq) \
             VALUES (?1, ?2, ?3, 'system', 'system', 'sys reply', ?4)",
            params![
                uuid::Uuid::new_v4().to_string(),
                channel_id,
                parent_id,
                max_seq + 1
            ],
        )
        .unwrap();
    }

    // Alice's inbox view: channel-level unread is still the top-level parent
    // (1), and thread_unread_count should count only the human reply (1), not
    // the system reply.
    let snapshot = store
        .get_history_snapshot("general", "alice", None, 10, None, None)
        .unwrap();
    // parent is alice's own → doesn't count; 1 human reply → 1. System reply excluded.
    // Verify via the view directly since thread_unread_count is on the view row.
    let conn = store.conn_for_test();
    let (chan_unread, thread_unread): (i64, i64) = conn
        .query_row(
            "SELECT unread_count, thread_unread_count
             FROM inbox_conversation_state_view
             WHERE conversation_name = 'general' AND member_name = 'alice'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    // Alice's own top-level message doesn't count for her; no other top-level from bob.
    assert_eq!(
        chan_unread, 0,
        "channel unread is 0 (alice authored the only top-level message)"
    );
    assert_eq!(
        thread_unread, 1,
        "thread_unread_count must count only bob's human reply, not the system reply"
    );
    // Keep the snapshot variable used so clippy is happy.
    let _ = snapshot;
}

#[test]
fn test_thread_notification_state_excludes_system_messages() {
    // Covers src/store/inbox.rs query. This test inserts a system thread
    // reply directly (same technique as the view test above) and verifies
    // that ThreadNotificationStateView.unread_count does not count it.
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store.create_human("bob").unwrap();
    store
        .join_channel("general", "bob", SenderType::Human)
        .unwrap();

    let parent_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "parent",
            attachment_ids: &[],
            run_id: None,
            suppress_event: false,
        })
        .unwrap();

    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_id),
            sender_name: "bob",
            sender_type: SenderType::Human,
            content: "bob reply",
            attachment_ids: &[],
            run_id: None,
            suppress_event: false,
        })
        .unwrap();

    // Direct insert: system thread reply.
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    {
        let conn = store.conn_for_test();
        let max_seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE channel_id = ?1",
                params![channel_id],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO messages (id, channel_id, thread_parent_id, sender_name, sender_type, content, seq) \
             VALUES (?1, ?2, ?3, 'system', 'system', 'sys reply', ?4)",
            params![
                uuid::Uuid::new_v4().to_string(),
                channel_id,
                parent_id,
                max_seq + 1
            ],
        )
        .unwrap();
    }

    let state = store
        .get_thread_notification_state("general", &parent_id, "alice")
        .unwrap()
        .unwrap();
    assert_eq!(
        state.unread_count, 1,
        "thread notification state must count only the human reply, not the system reply"
    );
}

#[test]
fn test_create_system_message_emits_system_typed_stream_event() {
    // Subscribe before posting to capture the stream event. Verifies the
    // payload's sender.type is "system" (not "human" as the legacy code wrote).
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let mut rx = store.subscribe();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    store
        .create_system_message(&channel_id, "Team assembled.")
        .unwrap();

    let event = rx
        .try_recv()
        .expect("stream event should be delivered synchronously");
    let payload = event.event_payload;
    let sender = payload.get("sender").expect("payload has sender field");
    let sender_type = sender
        .get("type")
        .and_then(|v| v.as_str())
        .expect("sender.type is a string");
    assert_eq!(
        sender_type, "system",
        "stream event must tag system messages with sender.type=system"
    );
    let sender_name = sender
        .get("name")
        .and_then(|v| v.as_str())
        .expect("sender.name is a string");
    assert_eq!(sender_name, "system");
}

#[test]
fn test_lookup_sender_type_recovers_after_mutex_poison() {
    use chorus::store::messages::SenderType;

    let store = Store::open(":memory:").unwrap();
    store.create_human("alice").unwrap();

    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _conn = store.conn_for_test();
        panic!("poison the store connection mutex");
    }));
    assert!(
        panic_result.is_err(),
        "expected intentional panic to poison mutex"
    );

    let sender_type = store.lookup_sender_type("alice").unwrap();
    assert_eq!(sender_type, Some(SenderType::Human));

    let _conn = store.conn_for_test();
}
