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
        .create_channel("general", Some("General"), ChannelType::Channel, None)
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
            None,
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
        .create_channel("ops-team", None, ChannelType::Team, None)
        .unwrap();
    store
        .create_team_member(&team_id, "bot1", "agent", &bot1.id, "operator")
        .unwrap();
    store
        .join_channel("ops-team", "bot1", SenderType::Agent)
        .unwrap();

    store
        .update_agent_record(&AgentRecordUpsert {
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
        .create_channel(
            "general",
            Some("General channel"),
            ChannelType::Channel,
            None,
        )
        .unwrap();
    store
        .create_channel("random", None, ChannelType::Channel, None)
        .unwrap();
    let channels = store.get_channels().unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].name, "general");
}

#[test]
fn test_send_and_receive_messages() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let msg_id = store
        .create_message(CreateMessage {
            channel_name: "general",
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
    let dm_channel_id = store.resolve_target("dm:@alice", "bot1").unwrap();
    let dm_channel = store.get_channel_by_id(&dm_channel_id).unwrap().unwrap();

    store
        .create_message(CreateMessage {
            channel_name: &dm_channel.name,
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
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    for i in 0..10 {
        store
            .create_message(CreateMessage {
                channel_name: "general",
                sender_name: "alice",
                sender_type: SenderType::Human,
                content: &format!("msg {i}"),
                attachment_ids: &[],
                suppress_event: false,
                run_id: None,
            })
            .unwrap();
    }

    let (msgs, has_more) = store.get_history("general", 5, None, None).unwrap();
    assert_eq!(msgs.len(), 5);
    assert!(has_more);

    let first_seq = msgs[0].seq;
    let (older, _) = store
        .get_history("general", 5, Some(first_seq), None)
        .unwrap();
    assert_eq!(older.len(), 5);
}

#[test]
fn test_history_snapshot_returns_messages_and_read_cursor() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    store
        .create_message(CreateMessage {
            channel_name: "general",
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
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "two",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let snapshot = store
        .get_history_snapshot("general", "alice", 10, None, None)
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
        .create_channel("general", None, ChannelType::Channel, None)
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
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "one",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let second_top_level = store
        .create_message(CreateMessage {
            channel_name: "general",
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
    assert_eq!(state_after.last_read_seq, 2);
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
        .create_channel("general", None, ChannelType::Channel, None)
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
        .get_history_snapshot("general", "bot1", 10, None, None)
        .unwrap();
    assert_eq!(snapshot_before.last_read_seq, 0);

    store.get_messages_for_agent("bot1", true).unwrap();

    let unread_after = store.get_unread_summary("bot1").unwrap();
    assert_eq!(unread_after.get("general"), None);

    let snapshot_after = store
        .get_history_snapshot("general", "bot1", 10, None, None)
        .unwrap();
    assert_eq!(snapshot_after.last_read_seq, 2);
    assert_eq!(store.get_last_read_seq("general", "bot1").unwrap(), 2);
}

#[test]
fn test_history_read_cursor_rejects_seq_above_max() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
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
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "b",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let err = store
        .set_history_read_cursor("general", "alice", SenderType::Human, 999)
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
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "a",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let err = store
        .set_history_read_cursor("general", "alice", SenderType::Human, -1)
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
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
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
        .set_history_read_cursor("general", "alice", SenderType::Human, 1)
        .unwrap();
    assert_eq!(store.get_last_read_seq("general", "alice").unwrap(), 1);
}

#[test]
fn test_conversation_messages_view_projects_message_rows() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let message_id = store
        .create_message(CreateMessage {
            channel_name: "general",
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
                    sender_name, sender_type, sender_deleted, content, seq
             FROM conversation_messages_view
             WHERE message_id = ?1",
            params![message_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(row.0, message_id);
    assert_eq!(row.1, channel.id);
    assert_eq!(row.2, "general");
    assert_eq!(row.3, "channel");
    assert_eq!(row.4, "alice");
    assert_eq!(row.5, "human");
    assert_eq!(row.6, 0);
    assert_eq!(row.7, "hello");
    assert_eq!(row.8, 1);
}

#[test]
fn test_agent_env_vars_persist_in_agent_record() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
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
        .update_agent_record(&AgentRecordUpsert {
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
        .create_channel("general", None, ChannelType::Channel, None)
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
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    store.mark_agent_messages_deleted("bot1").unwrap();
    let (history, _) = store.get_history("general", 10, None, None).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].sender_deleted);
}

#[test]
fn test_create_message_persists_top_level() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();

    let message_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    let (history, _) = store.get_history("general", 10, None, None).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, message_id);
    assert_eq!(history[0].content, "hello");
    assert_eq!(history[0].sender_name, "alice");
}

#[test]
fn test_unread_excludes_own_messages_for_sender() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
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
}

#[test]
fn test_tasks_crud() {
    let (store, _dir) = make_store();
    store
        .create_channel("eng", None, ChannelType::Channel, None)
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
        .create_channel("eng", None, ChannelType::Channel, None)
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
        .create_channel("general", None, ChannelType::Channel, None)
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

    let ch_id = store.resolve_target("#general", "bot1").unwrap();
    assert!(!ch_id.is_empty());

    let dm_id = store.resolve_target("dm:@alice", "bot1").unwrap();
    assert!(!dm_id.is_empty());
}

#[test]
fn test_list_channels_excludes_dm() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
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
        .create_channel("general", None, ChannelType::Channel, None)
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
    let dm_id = store.resolve_target("dm:@alice", "bot1").unwrap();

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

    let ch_id = store.resolve_target("dm:@alice", "bot1").unwrap();
    let ch_id2 = store.resolve_target("dm:@alice", "bot1").unwrap();
    assert_eq!(ch_id, ch_id2);
}

#[test]
fn test_update_channel_preserves_id_and_metadata() {
    let (store, dir) = make_store();
    let channel_id = store
        .create_channel(
            "general",
            Some("General channel"),
            ChannelType::Channel,
            None,
        )
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
        .create_channel(
            "general",
            Some("General channel"),
            ChannelType::Channel,
            None,
        )
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
        .create_channel(
            "general",
            Some("General channel"),
            ChannelType::Channel,
            None,
        )
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
        .create_channel("eng", Some("Engineering"), ChannelType::Channel, None)
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

    store
        .create_message(CreateMessage {
            channel_name: "eng",
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "hello",
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
        .create_channel("general", None, ChannelType::Channel, None)
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

    let (history, _) = store.get_history("general", 10, None, None).unwrap();
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
        .create_channel("general", None, ChannelType::Channel, None)
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
fn test_create_system_message_emits_system_typed_stream_event() {
    // Subscribe before posting to capture the stream event. Verifies the
    // payload's sender.type is "system" (not "human" as the legacy code wrote).
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
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

#[test]
fn task_event_payload_round_trips_through_json() {
    use chorus::store::tasks::events::{TaskEventAction, TaskEventPayload};
    use chorus::store::tasks::TaskStatus;

    let original = TaskEventPayload {
        action: TaskEventAction::Claimed,
        task_number: 7,
        title: "wire up the bridge".into(),
        sub_channel_id: "22222222-2222-2222-2222-222222222222".into(),
        actor: "alice".into(),
        prev_status: Some(TaskStatus::Todo),
        next_status: TaskStatus::InProgress,
        claimed_by: Some("alice".into()),
    };

    let json = original.to_json_string().unwrap();
    assert!(json.contains(r#""kind":"task_event""#));
    assert!(json.contains(r#""action":"claimed""#));
    assert!(json.contains(r#""nextStatus":"in_progress""#));
    assert!(json.contains(r#""taskNumber":7"#));

    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["kind"], "task_event");
    assert_eq!(parsed["actor"], "alice");
    assert_eq!(parsed["subChannelId"], "22222222-2222-2222-2222-222222222222");
}

#[test]
fn task_event_payload_serializes_none_fields_as_json_null() {
    use chorus::store::tasks::events::{TaskEventAction, TaskEventPayload};
    use chorus::store::tasks::TaskStatus;

    // Created events set prev_status = None and claimed_by = None. The frontend
    // parser needs `null` for these (not missing keys) to distinguish "event
    // explicitly cleared the field" from "producer forgot the field".
    let created = TaskEventPayload {
        action: TaskEventAction::Created,
        task_number: 1,
        title: "t".into(),
        sub_channel_id: "s".into(),
        actor: "a".into(),
        prev_status: None,
        next_status: TaskStatus::Todo,
        claimed_by: None,
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&created.to_json_string().unwrap()).unwrap();
    assert!(parsed["prevStatus"].is_null(), "expected JSON null, got {:?}", parsed["prevStatus"]);
    assert!(parsed["claimedBy"].is_null(), "expected JSON null, got {:?}", parsed["claimedBy"]);
    assert_eq!(parsed["action"], "created");
    assert_eq!(parsed["nextStatus"], "todo");
}

#[test]
fn create_tasks_emits_task_event_to_parent_channel() {
    let (store, _dir) = make_store();
    let parent_id = store
        .create_channel("eng", None, chorus::store::channels::ChannelType::Channel, None)
        .unwrap();
    store.create_human("bob").unwrap();
    store.join_channel("eng", "bob", chorus::store::messages::types::SenderType::Human).unwrap();

    let result = store
        .create_tasks("eng", "bob", &["wire up the bridge"])
        .unwrap();
    assert_eq!(result.len(), 1);

    let event_rows: Vec<(String, String)> = store
        .conn_for_test()
        .prepare("SELECT sender_type, content FROM messages WHERE channel_id = ?1 ORDER BY seq")
        .unwrap()
        .query_map(rusqlite::params![parent_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert_eq!(event_rows.len(), 1);
    assert_eq!(event_rows[0].0, "system");

    let parsed: serde_json::Value = serde_json::from_str(&event_rows[0].1).unwrap();
    assert_eq!(parsed["kind"], "task_event");
    assert_eq!(parsed["action"], "created");
    assert_eq!(parsed["actor"], "bob");
    assert_eq!(parsed["taskNumber"], 1);
    assert_eq!(parsed["nextStatus"], "todo");
    assert_eq!(parsed["title"], "wire up the bridge");
}
