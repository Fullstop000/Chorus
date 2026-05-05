mod harness;
use chorus::store::agents::AgentEnvVar;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, SenderType};
use chorus::store::tasks::TaskStatus;
use chorus::store::{AgentRecordUpsert, Store, WorkspaceMode};
use harness::join_channel_silent;
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
        .create_team_member(&team_id, "agent-uuid-1", "agent", "operator")
        .unwrap();
    store
        .create_team_member(&team_id, "bob", "human", "observer")
        .unwrap();
    let members = store.get_team_members(&team_id).unwrap();
    assert_eq!(members.len(), 2);
}

#[test]
fn test_list_teams_for_agent() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store
        .create_team_member(&team_id, "agent-uuid-1", "agent", "operator")
        .unwrap();
    let teams = store.get_teams_by_agent_id("agent-uuid-1").unwrap();
    assert_eq!(teams.len(), 1);
    assert_eq!(teams[0].team_name, "eng-team");
}

#[test]
fn test_delete_team_cascades() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store
        .create_team_member(&team_id, "uuid-1", "agent", "operator")
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
    // Tests pre-date the ID-first refactor and use bare names ("alice", "bob")
    // as both human ids and labels. Pre-seed those rows with id == name so the
    // store's `human not found` validations (e.g. `create_local_workspace`)
    // succeed without each test having to create the human itself.
    store.ensure_human_with_id("alice", "alice").unwrap();
    store.ensure_human_with_id("bob", "bob").unwrap();
    (store, dir)
}

#[test]
fn test_open_old_identity_schema_fails_loudly() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("legacy.db");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE humans (
                name TEXT PRIMARY KEY,
                display_name TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
    }

    let err = match Store::open(db_path.to_str().unwrap()) {
        Ok(_) => panic!("opening old identity schema should fail"),
        Err(err) => err,
    };
    let message = err.to_string();
    assert!(
        message.contains("old identity schema")
            && message.contains("humans.display_name")
            && message.contains("fresh data directory"),
        "unexpected error: {message}"
    );
}

#[test]
fn test_create_local_workspace_sets_owner_and_active_context() {
    let (store, _dir) = make_store();

    let (workspace, _event) = store.create_local_workspace("Chorus Dev", "alice").unwrap();

    assert_eq!(workspace.name, "Chorus Dev");
    assert_eq!(workspace.slug, "chorus-dev");
    assert_eq!(workspace.mode, WorkspaceMode::LocalOnly);
    assert_eq!(workspace.created_by_human_id.as_deref(), Some("alice"));
    assert_eq!(
        store.get_active_workspace().unwrap().unwrap().id,
        workspace.id
    );
    assert_eq!(
        store.list_workspaces_for_human("alice").unwrap()[0].id,
        workspace.id
    );
}

#[test]
fn test_create_local_workspace_provisions_all_channel() {
    let (store, _dir) = make_store();

    let (workspace, _event) = store.create_local_workspace("Chorus Dev", "alice").unwrap();

    let channels = store
        .get_channels_by_params(&chorus::store::ChannelListParams {
            workspace_id: Some(&workspace.id),
            include_system: true,
            ..Default::default()
        })
        .unwrap();
    let all = channels
        .iter()
        .find(|channel| channel.name == "all")
        .expect("workspace should have #all");
    assert_eq!(all.channel_type, ChannelType::System);
    assert!(store.channel_member_exists(&all.id, "alice").unwrap());
}

#[test]
fn test_scoped_resource_schema_requires_workspace_id() {
    let store = Store::open(":memory:").unwrap();
    let conn = store.conn_for_test();

    let channel_err = conn
        .execute(
            "INSERT INTO channels (id, name, channel_type)
             VALUES ('missing-workspace-channel', 'orphan', 'channel')",
            [],
        )
        .unwrap_err();
    assert!(channel_err
        .to_string()
        .contains("NOT NULL constraint failed: channels.workspace_id"));

    let agent_err = conn
        .execute(
            "INSERT INTO agents (id, name, display_name, runtime, model)
             VALUES ('missing-workspace-agent', 'orphan-agent', 'Orphan Agent', 'claude', 'sonnet')",
            [],
        )
        .unwrap_err();
    assert!(agent_err
        .to_string()
        .contains("NOT NULL constraint failed: agents.workspace_id"));

    let team_err = conn
        .execute(
            "INSERT INTO teams (id, name, display_name, collaboration_model)
             VALUES ('missing-workspace-team', 'orphan-team', 'Orphan Team', 'swarm')",
            [],
        )
        .unwrap_err();
    assert!(team_err
        .to_string()
        .contains("NOT NULL constraint failed: teams.workspace_id"));
}

#[test]
fn test_workspace_scoped_names_allow_same_channel_name_in_different_workspaces() {
    let (store, _dir) = make_store();
    let (alpha, _event) = store.create_local_workspace("Alpha", "alice").unwrap();
    let (beta, _event) = store.create_local_workspace("Beta", "bob").unwrap();

    store
        .create_channel_in_workspace(
            &alpha.id,
            "general",
            Some("Alpha general"),
            ChannelType::Channel,
            None,
        )
        .unwrap();
    store
        .create_channel_in_workspace(
            &beta.id,
            "general",
            Some("Beta general"),
            ChannelType::Channel,
            None,
        )
        .unwrap();

    assert!(store
        .create_channel_in_workspace(
            &alpha.id,
            "general",
            Some("Duplicate Alpha general"),
            ChannelType::Channel,
            None,
        )
        .is_err());
}

#[test]
fn test_unknown_workspace_mode_surfaces_error() {
    let (store, _dir) = make_store();
    {
        let conn = store.conn_for_test();
        conn.execute(
            "INSERT INTO workspaces (id, name, slug, mode, created_by_human_id)
             VALUES ('workspace-1', 'Broken', 'broken', 'surprise', 'alice')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO local_workspace_state (key, workspace_id)
             VALUES ('active_workspace_id', 'workspace-1')",
            [],
        )
        .unwrap();
    }

    let err = store.get_active_workspace().unwrap_err();

    assert!(err.to_string().contains("unknown workspace mode"));
}

#[test]
fn test_workspace_selector_switch_rename_and_slug_collision() {
    let (store, _dir) = make_store();
    let (first, _event) = store.create_local_workspace("Acme", "alice").unwrap();
    let (second, _event) = store.create_local_workspace("Acme", "alice").unwrap();

    assert_eq!(first.slug, "acme");
    assert_eq!(second.slug, "acme-1");
    assert_eq!(
        store.get_workspace_by_selector("acme").unwrap().unwrap().id,
        first.id
    );

    store.set_active_workspace(&first.id).unwrap();
    assert_eq!(store.get_active_workspace().unwrap().unwrap().id, first.id);

    let renamed = store.rename_workspace(&first.id, "Acme Renamed").unwrap();
    assert_eq!(renamed.name, "Acme Renamed");
    assert_eq!(renamed.slug, "acme");
}

#[test]
fn test_workspace_scoped_core_resource_lists() {
    let (store, _dir) = make_store();
    let (alpha, _event) = store.create_local_workspace("Alpha", "alice").unwrap();
    let (beta, _event) = store.create_local_workspace("Beta", "alice").unwrap();

    store
        .create_channel_in_workspace(
            &alpha.id,
            "general",
            Some("Alpha general"),
            ChannelType::Channel,
            None,
        )
        .unwrap();
    store
        .create_channel_in_workspace(
            &beta.id,
            "general",
            Some("Beta general"),
            ChannelType::Channel,
            None,
        )
        .unwrap();
    store
        .create_agent_record_in_workspace(
            &alpha.id,
            &AgentRecordUpsert {
                name: "alpha-bot",
                display_name: "Alpha Bot",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                machine_id: None,
                env_vars: &[],
            },
        )
        .unwrap();
    store
        .create_agent_record_in_workspace(
            &beta.id,
            &AgentRecordUpsert {
                name: "beta-bot",
                display_name: "Beta Bot",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                machine_id: None,
                env_vars: &[],
            },
        )
        .unwrap();
    store
        .create_team_in_workspace(&alpha.id, "alpha-team", "Alpha Team", "swarm", None)
        .unwrap();
    store
        .create_team_in_workspace(&beta.id, "beta-team", "Beta Team", "swarm", None)
        .unwrap();

    let alpha_channels = store
        .get_channels_by_params(&chorus::store::ChannelListParams {
            workspace_id: Some(&alpha.id),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(alpha_channels.len(), 1);
    assert_eq!(
        alpha_channels[0].description.as_deref(),
        Some("Alpha general")
    );

    let alpha_agents = store.get_agents_in_workspace(&alpha.id).unwrap();
    assert_eq!(alpha_agents.len(), 1);
    assert_eq!(alpha_agents[0].name, "alpha-bot");

    let alpha_teams = store.get_teams_in_workspace(&alpha.id).unwrap();
    assert_eq!(alpha_teams.len(), 1);
    assert_eq!(alpha_teams[0].name, "alpha-team");
}

#[test]
fn test_workspace_scoped_team_channel_join_uses_workspace_id() {
    let (store, _dir) = make_store();
    let (alpha, _event) = store.create_local_workspace("Alpha", "alice").unwrap();
    let (beta, _event) = store.create_local_workspace("Beta", "alice").unwrap();

    let alpha_channel_id = store
        .create_channel_in_workspace(&alpha.id, "ops", None, ChannelType::Team, None)
        .unwrap();
    store
        .create_channel_in_workspace(&beta.id, "ops", None, ChannelType::Team, None)
        .unwrap();
    store
        .create_team_in_workspace(&alpha.id, "ops", "Alpha Ops", "swarm", None)
        .unwrap();
    store
        .create_team_in_workspace(&beta.id, "ops", "Beta Ops", "swarm", None)
        .unwrap();

    let alpha_teams = store.get_teams_in_workspace(&alpha.id).unwrap();
    assert_eq!(alpha_teams.len(), 1);
    assert_eq!(alpha_teams[0].display_name, "Alpha Ops");
    assert_eq!(
        alpha_teams[0].channel_id.as_deref(),
        Some(alpha_channel_id.as_str())
    );
}

#[test]
fn test_team_with_channel_create_rolls_back_when_channel_insert_fails() {
    let (store, _dir) = make_store();
    store
        .create_channel("ops", None, ChannelType::Team, None)
        .unwrap();

    let err = store
        .create_team_with_channel("ops", "Ops", "swarm", None)
        .unwrap_err();
    assert!(
        err.to_string().contains("UNIQUE constraint"),
        "expected channel uniqueness failure, got: {err}"
    );

    let team_count: i64 = store
        .conn_for_test()
        .query_row("SELECT COUNT(*) FROM teams WHERE name = 'ops'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(team_count, 0, "team row should roll back with channel row");
}

#[test]
fn test_compat_agent_and_team_helpers_write_active_workspace_rows() {
    let (store, _dir) = make_store();
    let (alpha, _event) = store.create_local_workspace("Alpha", "alice").unwrap();

    store
        .create_agent_record(&AgentRecordUpsert {
            name: "compat-bot",
            display_name: "Compat Bot",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .create_agent_record_in_workspace(
            &alpha.id,
            &AgentRecordUpsert {
                name: "alpha-bot",
                display_name: "Alpha Bot",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                machine_id: None,
                env_vars: &[],
            },
        )
        .unwrap();
    store
        .create_team("compat-team", "Compat Team", "swarm", None)
        .unwrap();
    store
        .create_team_in_workspace(&alpha.id, "alpha-team", "Alpha Team", "swarm", None)
        .unwrap();

    let agents = store.get_agents().unwrap();
    assert_eq!(agents.len(), 2);
    assert!(agents.iter().all(|agent| agent.workspace_id == alpha.id));
    assert!(agents.iter().any(|agent| agent.name == "compat-bot"));
    assert!(agents.iter().any(|agent| agent.name == "alpha-bot"));

    let teams = store.get_teams().unwrap();
    assert_eq!(teams.len(), 2);
    assert!(teams.iter().all(|team| team.workspace_id == alpha.id));
    assert!(teams.iter().any(|team| team.name == "compat-team"));
    assert!(teams.iter().any(|team| team.name == "alpha-team"));
}

#[test]
fn test_delete_workspace_wipes_scoped_data_and_keeps_other_workspaces() {
    let (store, dir) = make_store();
    let (alpha, _event) = store.create_local_workspace("Alpha", "alice").unwrap();
    let (beta, _event) = store.create_local_workspace("Beta", "bob").unwrap();
    let alpha_channel_id = store
        .create_channel_in_workspace(&alpha.id, "alpha-general", None, ChannelType::Channel, None)
        .unwrap();
    let beta_channel_id = store
        .create_channel_in_workspace(&beta.id, "beta-general", None, ChannelType::Channel, None)
        .unwrap();
    let alpha_agent_id = store
        .create_agent_record_in_workspace(
            &alpha.id,
            &AgentRecordUpsert {
                name: "alpha-bot",
                display_name: "Alpha Bot",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                machine_id: None,
                env_vars: &[AgentEnvVar {
                    key: "TOKEN".to_string(),
                    value: "secret".to_string(),
                    position: 0,
                }],
            },
        )
        .unwrap();
    let (alpha_team_id, _) = store
        .create_team_with_channel_in_workspace(&alpha.id, "alpha-team", "Alpha Team", "swarm", None)
        .unwrap();
    store
        .create_team_member(&alpha_team_id, &alpha_agent_id, "agent", "member")
        .unwrap();

    let alpha_attachment_path = dir.path().join("alpha-attachment.txt");
    let beta_attachment_path = dir.path().join("beta-attachment.txt");
    std::fs::write(&alpha_attachment_path, "alpha").unwrap();
    std::fs::write(&beta_attachment_path, "beta").unwrap();
    {
        let conn = store.conn_for_test();
        conn.execute(
            "INSERT INTO agent_sessions (agent_id, session_id, runtime)
             VALUES (?1, 'session-alpha', 'claude')",
            params![alpha_agent_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channels (id, workspace_id, name, channel_type, parent_channel_id)
             VALUES ('alpha-task-channel', ?1, 'alpha-general__task-1', 'task', ?2)",
            params![alpha.id, alpha_channel_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channel_members (channel_id, member_id, member_type)
             VALUES (?1, 'alice', 'human')",
            params![alpha_channel_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channel_members (channel_id, member_id, member_type)
             VALUES ('alpha-task-channel', 'alice', 'human')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO inbox_read_state (conversation_id, member_id, member_type)
             VALUES (?1, 'alice', 'human')",
            params![alpha_channel_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, channel_id, task_number, title, created_by_id, created_by_type, sub_channel_id)
             VALUES ('alpha-task', ?1, 1, 'Ship alpha', 'alice', 'human', 'alpha-task-channel')",
            params![alpha_channel_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, channel_id, sender_id, sender_type, content, seq, run_id)
             VALUES ('alpha-message', ?1, 'alice', 'human', 'alpha', 1, 'run-alpha')",
            params![alpha_channel_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trace_events (run_id, seq, timestamp_ms, kind, data)
             VALUES ('run-alpha', 1, 1, 'text', '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments (id, filename, mime_type, size_bytes, stored_path)
             VALUES ('alpha-attachment', 'alpha.txt', 'text/plain', 5, ?1)",
            params![alpha_attachment_path.to_string_lossy().as_ref()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message_attachments (message_id, attachment_id)
             VALUES ('alpha-message', 'alpha-attachment')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, channel_id, sender_id, sender_type, content, seq, run_id)
             VALUES ('beta-message', ?1, 'bob', 'human', 'beta', 1, 'run-beta')",
            params![beta_channel_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trace_events (run_id, seq, timestamp_ms, kind, data)
             VALUES ('run-beta', 1, 1, 'text', '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments (id, filename, mime_type, size_bytes, stored_path)
             VALUES ('beta-attachment', 'beta.txt', 'text/plain', 4, ?1)",
            params![beta_attachment_path.to_string_lossy().as_ref()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message_attachments (message_id, attachment_id)
             VALUES ('beta-message', 'beta-attachment')",
            [],
        )
        .unwrap();
    }

    store.delete_workspace(&alpha.id).unwrap();

    let conn = store.conn_for_test();
    for (table, predicate) in [
        ("workspaces", format!("id = '{}'", alpha.id)),
        (
            "workspace_members",
            format!("workspace_id = '{}'", alpha.id),
        ),
        (
            "local_workspace_state",
            format!("workspace_id = '{}'", alpha.id),
        ),
        ("channels", "id IN ('alpha-task-channel')".to_string()),
        ("channels", format!("workspace_id = '{}'", alpha.id)),
        (
            "channel_members",
            format!("channel_id = '{}'", alpha_channel_id),
        ),
        ("messages", "id = 'alpha-message'".to_string()),
        ("tasks", "id = 'alpha-task'".to_string()),
        (
            "inbox_read_state",
            format!("conversation_id = '{}'", alpha_channel_id),
        ),
        ("trace_events", "run_id = 'run-alpha'".to_string()),
        (
            "message_attachments",
            "attachment_id = 'alpha-attachment'".to_string(),
        ),
        ("attachments", "id = 'alpha-attachment'".to_string()),
        ("agents", "name = 'alpha-bot'".to_string()),
        ("agent_env_vars", "agent_name = 'alpha-bot'".to_string()),
        ("agent_sessions", format!("agent_id = '{}'", alpha_agent_id)),
        ("teams", "name = 'alpha-team'".to_string()),
        ("team_members", format!("team_id = '{}'", alpha_team_id)),
    ] {
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {predicate}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table} rows should be deleted");
    }

    let beta_workspace_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspaces WHERE id = ?1",
            params![beta.id],
            |row| row.get(0),
        )
        .unwrap();
    let beta_message_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE id = 'beta-message'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let beta_attachment_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attachments WHERE id = 'beta-attachment'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(beta_workspace_count, 1);
    assert_eq!(beta_message_count, 1);
    assert_eq!(beta_attachment_count, 1);
    assert!(!alpha_attachment_path.exists());
    assert!(beta_attachment_path.exists());
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    join_channel_silent(&store, "general", "bot1", "agent");
    let bot1 = store.get_agent("bot1").unwrap().unwrap();

    store
        .create_channel(
            "ops-room",
            Some("Shell API mutation coverage"),
            ChannelType::Channel,
            None,
        )
        .unwrap();
    join_channel_silent(&store, "ops-room", "alice", "human");
    let channel_id = store.get_channel_by_name("ops-room").unwrap().unwrap().id;
    store.archive_channel(&channel_id).unwrap();

    let team_id = store
        .create_team("ops-team", "Ops Team", "leader_operators", Some("bot1"))
        .unwrap();
    store
        .create_channel("ops-team", None, ChannelType::Team, None)
        .unwrap();
    store
        .create_team_member(&team_id, &bot1.id, "agent", "operator")
        .unwrap();
    join_channel_silent(&store, "ops-team", "bot1", "agent");

    store
        .update_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Ops Bot",
            description: Some("Updated from shell mutation test"),
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");

    let msg_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();
    assert!(!msg_id.is_empty());

    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();

    // Before joining any channels, the agent has no inbox messages.
    let msgs = store.get_messages_for_agent_id(&bot1_id, false).unwrap();
    assert!(msgs.is_empty());

    join_channel_silent(&store, "general", &bot1_id, "agent");

    let _msg_id2 = store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "hello bot",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();
    let msgs = store.get_messages_for_agent_id(&bot1_id, false).unwrap();
    assert_eq!(msgs.len(), 2);
}

#[test]
fn test_agent_does_not_receive_its_own_sent_message() {
    let (store, _dir) = make_store();
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    let dm_channel_id = store.resolve_target("dm:@alice", &bot1_id).unwrap();
    let dm_channel = store.get_channel_by_id(&dm_channel_id).unwrap().unwrap();

    store
        .create_message(CreateMessage {
            channel_name: &dm_channel.name,
            sender_id: &bot1_id,
            sender_type: SenderType::Agent,
            content: "hello alice",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();

    let unread = store.get_messages_for_agent_id(&bot1_id, false).unwrap();
    assert!(
        unread.is_empty(),
        "an agent should not get its own outbound message back as unread"
    );

    let last_read = store.get_last_read_seq(&dm_channel.name, &bot1_id).unwrap();
    assert_eq!(last_read, 1, "sender read position should advance on send");
}

#[test]
fn test_message_history_pagination() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");

    for i in 0..10 {
        store
            .create_message(CreateMessage {
                channel_name: "general",
                sender_id: "alice",
                sender_type: SenderType::Human,
                content: &format!("msg {i}"),
                attachment_ids: &[],
                suppress_event: false,
                run_id: None,
            })
            .map(|(id, _)| id)
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");

    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "one",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "two",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    join_channel_silent(&store, "general", &bot1_id, "agent");

    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "one",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();
    let second_top_level = store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "two",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();

    let state_before = store
        .get_inbox_conversation_state("general", &bot1_id)
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
             WHERE conversation_name = 'general' AND member_id = ?1",
            params![bot1_id],
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

    let unread = store.get_messages_for_agent_id(&bot1_id, true).unwrap();
    assert_eq!(unread.len(), 2);

    let state_after = store
        .get_inbox_conversation_state("general", &bot1_id)
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
             WHERE channel_id = ?1 AND member_id = ?2",
            params![state_after.conversation_id, bot1_id],
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    join_channel_silent(&store, "general", &bot1_id, "agent");

    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "one",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "two",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();

    let unread_before = store.get_unread_summary(&bot1_id).unwrap();
    assert_eq!(unread_before.get("general"), Some(&2));

    let snapshot_before = store
        .get_history_snapshot("general", &bot1_id, 10, None, None)
        .unwrap();
    assert_eq!(snapshot_before.last_read_seq, 0);

    store.get_messages_for_agent_id(&bot1_id, true).unwrap();

    let unread_after = store.get_unread_summary(&bot1_id).unwrap();
    assert_eq!(unread_after.get("general"), None);

    let snapshot_after = store
        .get_history_snapshot("general", &bot1_id, 10, None, None)
        .unwrap();
    assert_eq!(snapshot_after.last_read_seq, 2);
    assert_eq!(store.get_last_read_seq("general", &bot1_id).unwrap(), 2);
}

#[test]
fn test_history_read_cursor_rejects_seq_above_max() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "a",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "b",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "a",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "a",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();

    let channel = store.get_channel_by_name("general").unwrap().unwrap();
    {
        let conn = store.conn_for_test();
        conn.execute(
            "INSERT INTO inbox_read_state (
                conversation_id, member_id, member_type, last_read_seq, last_read_message_id, updated_at
             ) VALUES (?1, 'alice', 'human', 50, NULL, datetime('now'))
             ON CONFLICT(conversation_id, member_type, member_id) DO UPDATE SET last_read_seq = excluded.last_read_seq",
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");

    let message_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
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
            machine_id: None,
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
            machine_id: None,
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
            machine_id: None,
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
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: &bot1_id,
            sender_type: SenderType::Agent,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");

    let message_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    join_channel_silent(&store, "general", &bot1_id, "agent");

    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: &bot1_id,
            sender_type: SenderType::Agent,
            content: "from bot a",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: &bot1_id,
            sender_type: SenderType::Agent,
            content: "from bot b",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
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
        .get_inbox_conversation_state("general", &bot1_id)
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
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();

    let (tasks, _events) = store
        .create_tasks(
            "eng",
            &bot1_id,
            SenderType::Agent,
            &["Fix bug", "Add feature"],
        )
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
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    let bot2_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "o3",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .create_tasks("eng", &bot1_id, SenderType::Agent, &["Task A"])
        .map(|(tasks, _)| tasks)
        .unwrap();

    let (results, _events) = store
        .update_tasks_claim("eng", &bot1_id, SenderType::Agent, &[1])
        .unwrap();
    assert!(results[0].success);

    let (results, _events) = store
        .update_tasks_claim("eng", &bot2_id, SenderType::Agent, &[1])
        .unwrap();
    assert!(!results[0].success);

    store
        .update_task_status("eng", 1, &bot1_id, SenderType::Agent, TaskStatus::InReview)
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
    store.ensure_human_with_id("carol-id", "carol").unwrap();
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();

    let ch_id = store.resolve_target("#general", &bot1_id).unwrap();
    assert!(!ch_id.is_empty());

    assert!(store.resolve_target("dm:@carol", &bot1_id).is_err());

    let dm_id = store.resolve_target("dm:@carol-id", &bot1_id).unwrap();
    assert!(!dm_id.is_empty());
}

#[test]
fn test_list_channels_excludes_dm() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    // Create a DM channel via resolve_target
    store.resolve_target("dm:@alice", &bot1_id).unwrap();

    let channels = store.get_channels().unwrap();
    assert_eq!(
        channels.len(),
        1,
        "list_channels must not return DM channels"
    );
    assert_eq!(channels[0].name, "general");
}

#[test]
fn test_dm_channels() {
    let (store, _dir) = make_store();
    store.ensure_human_with_id("alice", "alice").unwrap();
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();

    let ch_id = store.resolve_target("dm:@alice", &bot1_id).unwrap();
    let ch_id2 = store.resolve_target("dm:@alice", &bot1_id).unwrap();
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");

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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");

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
    store.ensure_human_with_id("alice", "alice").unwrap();
    store.ensure_human_with_id("zoe", "zoe").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
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
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();

    store.ensure_builtin_channels("alice").unwrap();

    let all = store.get_channel_by_name("all").unwrap().unwrap();
    let profiles = store.get_channel_member_profiles(&all.id).unwrap();
    let names: Vec<_> = profiles
        .iter()
        .map(|profile| profile.member_name.as_str())
        .collect();
    assert!(names.contains(&"alice"));
    assert!(names.contains(&"zoe"));
    assert!(names.contains(&"bot1"));
    assert!(names.contains(&"bot2"));
}

#[test]
fn test_ensure_builtin_channels_only_exposes_all_system_channel() {
    let (store, _dir) = make_store();
    store.ensure_human_with_id("alice", "alice").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
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
fn test_new_agents_auto_join_all_when_it_exists() {
    // Humans currently rely on `ensure_builtin_channels` for backfill; new
    // humans created via `ensure_human_with_id` are not automatically joined
    // to `#all`. Agents, however, are auto-joined when their record is
    // created (see `create_agent_record_inner`). This test pins that
    // agent-side guarantee.
    let (store, _dir) = make_store();
    store.ensure_human_with_id("alice", "alice").unwrap();
    store.ensure_builtin_channels("alice").unwrap();

    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();

    assert!(store.is_member("all", "alice").unwrap());
    assert!(store.is_member("all", &bot1_id).unwrap());
}

#[test]
fn test_ensure_builtin_channels_repairs_active_workspace_all() {
    let (store, _dir) = make_store();
    let (workspace, _event) = store
        .create_local_workspace("Chorus Local", "alice")
        .unwrap();

    {
        let conn = store.conn_for_test();
        conn.execute(
            "DELETE FROM channel_members
             WHERE channel_id IN (SELECT id FROM channels WHERE workspace_id = ?1 AND name = 'all')",
            params![workspace.id],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM channels WHERE workspace_id = ?1 AND name = 'all'",
            params![workspace.id],
        )
        .unwrap();
    }
    store
        .create_channel("all", Some("Recovered all"), ChannelType::System, None)
        .unwrap();
    let bot1_id = store
        .create_agent_record_in_workspace(
            &workspace.id,
            &AgentRecordUpsert {
                name: "bot1",
                display_name: "Bot 1",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                machine_id: None,
                env_vars: &[],
            },
        )
        .unwrap();

    store.ensure_builtin_channels("alice").unwrap();

    let auto_join_channels = store
        .get_auto_join_channels_for_workspace(Some(&workspace.id))
        .unwrap();
    assert_eq!(auto_join_channels.len(), 1);
    let all = &auto_join_channels[0];
    assert_eq!(all.name, "all");
    assert_eq!(all.workspace_id, workspace.id);
    assert!(store.channel_member_exists(&all.id, "alice").unwrap());
    assert!(store.channel_member_exists(&all.id, &bot1_id).unwrap());
}

#[test]
fn test_delete_channel_removes_messages_tasks_and_memberships() {
    let (store, dir) = make_store();
    let channel_id = store
        .create_channel("eng", Some("Engineering"), ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "eng", "alice", "human");
    let bot1_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    join_channel_silent(&store, "eng", &bot1_id, "agent");

    store
        .create_message(CreateMessage {
            channel_name: "eng",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();
    store
        .create_tasks("eng", &bot1_id, SenderType::Agent, &["ship it"])
        .map(|(tasks, _)| tasks)
        .unwrap();

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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");

    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    let (msg_id, _event) = store
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");
    store.ensure_human_with_id("bob", "bob").unwrap();
    join_channel_silent(&store, "general", "bob", "human");

    // A regular message from bob — alice should see 1 unread.
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "bob",
            sender_type: SenderType::Human,
            content: "hey",
            attachment_ids: &[],
            run_id: None,
            suppress_event: false,
        })
        .map(|(id, _)| id)
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
        .map(|(id, _)| id)
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
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "general", "alice", "human");

    let event_bus = chorus::server::event_bus::EventBus::new();
    let mut rx = event_bus.subscribe();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    let (_msg_id, event) = store
        .create_system_message(&channel_id, "Team assembled.")
        .unwrap();
    event_bus.publish_stream(event);

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
fn test_join_channel_creates_notice_and_is_idempotent() {
    let (store, _dir) = make_store();
    store
        .create_channel("general", None, ChannelType::Channel, None)
        .unwrap();

    let channel = store.get_channel_by_name("general").unwrap().unwrap();

    // Human joins — creates system message.
    let (joined, _events) = store
        .join_channel_by_id(&channel.id, "alice", SenderType::Human)
        .unwrap();
    assert!(joined, "first join should return true");

    let (history, _) = store.get_history("general", 10, None, None).unwrap();
    let sys_msg = history
        .iter()
        .find(|m| m.sender_type == "system")
        .expect("system message should appear in history");
    assert_eq!(sys_msg.content, "alice joined #general");
    let alice_payload = sys_msg
        .payload
        .as_ref()
        .expect("structured payload should be populated for member_joined");
    assert_eq!(alice_payload["kind"], "member_joined");
    assert_eq!(alice_payload["audience"], "humans");
    assert_eq!(alice_payload["actor"]["id"], "alice");
    assert_eq!(alice_payload["actor"]["type"], "human");
    assert_eq!(alice_payload["verb"], "joined");
    assert_eq!(alice_payload["target"]["id"], channel.id);
    assert_eq!(alice_payload["target"]["type"], "channel");
    assert_eq!(alice_payload["target"]["label"], "#general");

    // Idempotent re-join — no duplicate system message.
    let (joined_again, _events) = store
        .join_channel_by_id(&channel.id, "alice", SenderType::Human)
        .unwrap();
    assert!(!joined_again, "re-join should return false");

    let (history2, _) = store.get_history("general", 10, None, None).unwrap();
    let sys_count = history2
        .iter()
        .filter(|m| m.sender_type == "system")
        .count();
    assert_eq!(sys_count, 1, "only one system message should exist");

    // Agent joins — system message includes "Agent" prefix; notice carries
    // the agent's id and Agent actor type so the chip routes to the agent
    // profile, not a phantom human row.
    let bot_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot One",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    let (bot_joined, _events) = store
        .join_channel_by_id(&channel.id, &bot_id, SenderType::Agent)
        .unwrap();
    assert!(bot_joined, "agent first join should return true");

    let (history3, _) = store.get_history("general", 10, None, None).unwrap();
    let bot_sys_msg = history3
        .iter()
        .find(|m| m.content.contains("Bot One"))
        .expect("agent join system message should appear");
    assert_eq!(bot_sys_msg.content, "Agent Bot One joined #general");
    let bot_payload = bot_sys_msg
        .payload
        .as_ref()
        .expect("agent join should carry structured payload");
    assert_eq!(bot_payload["actor"]["id"], bot_id);
    assert_eq!(bot_payload["actor"]["type"], "agent");

    // Human with a UUID-style id (not matching name) resolves label correctly.
    store
        .ensure_human_with_id("human_carol_123", "carol")
        .unwrap();
    let (carol_joined, _events) = store
        .join_channel_by_id(&channel.id, "human_carol_123", SenderType::Human)
        .unwrap();
    assert!(carol_joined, "UUID-id human first join should return true");

    let (history4, _) = store.get_history("general", 10, None, None).unwrap();
    let carol_sys_msg = history4
        .iter()
        .find(|m| m.content.contains("carol"))
        .expect("UUID-id human join system message should appear");
    assert_eq!(carol_sys_msg.content, "carol joined #general");
    let carol_payload = carol_sys_msg
        .payload
        .as_ref()
        .expect("UUID-id human join should carry structured payload");
    assert_eq!(carol_payload["actor"]["id"], "human_carol_123");
}

#[test]
fn agent_read_paths_exclude_humans_only_payloads_but_ui_keeps_them() {
    // member_joined payloads carry `audience: "humans"` because they're
    // visual ambient markers for the human chat UI. Surfacing them to
    // agents creates noise: every join would wake every agent in the
    // channel and dump a `"alice joined #x"` line into their context.
    // Filter is structural — `payload.audience != 'humans'`, not a
    // kind allowlist — so task events (no audience field, defaults to
    // `all`) keep flowing.
    let (store, _dir) = make_store();
    store
        .create_channel("crew", None, ChannelType::Channel, None)
        .unwrap();
    let channel = store.get_channel_by_name("crew").unwrap().unwrap();

    // Agent joins the channel first so it has a membership row + read cursor.
    let agent_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "scout",
            display_name: "Scout",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    join_channel_silent(&store, "crew", &agent_id, "agent");

    // Human joins after the agent — this writes a `member_joined` payload
    // tagged `audience: "humans"` that the agent should NOT see when it polls.
    let (joined, _events) = store
        .join_channel_by_id(&channel.id, "alice", SenderType::Human)
        .unwrap();
    assert!(joined);

    // Inject a regular user message + a task to prove the filter is
    // structural (audience-driven), not a kind allowlist: regular content
    // and task events still flow.
    store
        .create_message(CreateMessage {
            channel_name: "crew",
            sender_id: "alice",
            sender_type: SenderType::Human,
            content: "ping",
            attachment_ids: &[],
            suppress_event: true,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();
    store
        .create_tasks("crew", "alice", SenderType::Human, &["ship"])
        .map(|(tasks, _)| tasks)
        .unwrap();

    // Agent receive — must skip the join chip but include alice's ping
    // and the task-event row.
    let received = store.get_messages_for_agent_id(&agent_id, false).unwrap();
    assert!(
        received.iter().all(|m| !m.content.contains("alice joined")),
        "humans-only payload must NOT reach the agent receive path; got: {:?}",
        received.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
    assert!(
        received.iter().any(|m| m.content == "ping"),
        "regular human message should still flow to the agent"
    );
    assert!(
        received
            .iter()
            .any(|m| m.content.contains("created #1") && m.content.contains("ship")),
        "task-event system messages should still flow to the agent (audience defaults to all)"
    );

    // Agent read_history (the bridge tool) must apply the same filter.
    let agent_snapshot = store
        .get_history_snapshot_for_agent("crew", &agent_id, 50, None, None)
        .unwrap();
    assert!(
        agent_snapshot.messages.iter().all(|m| m
            .payload
            .as_ref()
            .and_then(|p| p.get("audience"))
            .and_then(|a| a.as_str())
            != Some("humans")),
        "get_history_snapshot_for_agent must hide all rows tagged audience=humans"
    );
    assert!(
        agent_snapshot.messages.iter().any(|m| m
            .payload
            .as_ref()
            .and_then(|p| p.get("kind"))
            .and_then(|k| k.as_str())
            == Some("task_event")),
        "task events must still appear in agent history"
    );

    // The UI history endpoint preserves the humans-only payload — the
    // whole point of the structured chip is that the human chat renders it.
    let ui_snapshot = store
        .get_history_snapshot("crew", "alice", 50, None, None)
        .unwrap();
    let ui_join_msg = ui_snapshot
        .messages
        .iter()
        .find(|m| m.content == "alice joined #crew")
        .expect("UI history must keep the member_joined payload");
    assert!(
        ui_join_msg.payload.is_some(),
        "UI history should carry the structured payload for chip rendering"
    );
}

#[test]
fn test_lookup_sender_type_recovers_after_mutex_poison() {
    use chorus::store::messages::SenderType;

    let store = Store::open(":memory:").unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();

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

    let parsed = original.to_json_value();
    assert_eq!(parsed["kind"], "task_event");
    assert_eq!(parsed["action"], "claimed");
    assert_eq!(parsed["nextStatus"], "in_progress");
    assert_eq!(parsed["taskNumber"], 7);
    assert_eq!(parsed["actor"], "alice");
    assert_eq!(
        parsed["subChannelId"],
        "22222222-2222-2222-2222-222222222222"
    );
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
    let parsed = created.to_json_value();
    assert!(
        parsed["prevStatus"].is_null(),
        "expected JSON null, got {:?}",
        parsed["prevStatus"]
    );
    assert!(
        parsed["claimedBy"].is_null(),
        "expected JSON null, got {:?}",
        parsed["claimedBy"]
    );
    assert_eq!(parsed["action"], "created");
    assert_eq!(parsed["nextStatus"], "todo");
}

#[test]
fn create_tasks_emits_task_event_to_parent_channel() {
    let (store, _dir) = make_store();
    let parent_id = store
        .create_channel(
            "eng",
            None,
            chorus::store::channels::ChannelType::Channel,
            None,
        )
        .unwrap();
    store.ensure_human_with_id("bob", "bob").unwrap();
    join_channel_silent(&store, "eng", "bob", "human");

    let (result, _events) = store
        .create_tasks("eng", "bob", SenderType::Human, &["wire up the bridge"])
        .unwrap();
    assert_eq!(result.len(), 1);

    let event_rows: Vec<(String, String, String)> = store
        .conn_for_test()
        .prepare(
            "SELECT sender_type, content, payload FROM messages \
             WHERE channel_id = ?1 ORDER BY seq",
        )
        .unwrap()
        .query_map(rusqlite::params![parent_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert_eq!(event_rows.len(), 1);
    assert_eq!(event_rows[0].0, "system");
    assert_eq!(event_rows[0].1, "bob created #1 \"wire up the bridge\"");

    let parsed: serde_json::Value = serde_json::from_str(&event_rows[0].2).unwrap();
    assert_eq!(parsed["kind"], "task_event");
    assert_eq!(parsed["action"], "created");
    assert_eq!(parsed["actor"], "bob");
    assert_eq!(parsed["taskNumber"], 1);
    assert_eq!(parsed["nextStatus"], "todo");
    assert_eq!(parsed["title"], "wire up the bridge");
}

#[test]
fn claim_task_emits_claimed_event_to_parent_channel() {
    let (store, _dir) = make_store();
    let parent_id = store
        .create_channel(
            "eng",
            None,
            chorus::store::channels::ChannelType::Channel,
            None,
        )
        .unwrap();
    store.ensure_human_with_id("bob", "bob").unwrap();
    join_channel_silent(&store, "eng", "bob", "human");
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "eng", "alice", "human");

    store
        .create_tasks("eng", "bob", SenderType::Human, &["t"])
        .map(|(tasks, _)| tasks)
        .unwrap();
    store
        .update_tasks_claim("eng", "alice", SenderType::Human, &[1])
        .map(|(results, _)| results)
        .unwrap();

    let events: Vec<serde_json::Value> = store
        .conn_for_test()
        .prepare("SELECT payload FROM messages WHERE channel_id = ?1 AND sender_type = 'system' ORDER BY seq")
        .unwrap()
        .query_map(rusqlite::params![parent_id], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|s| serde_json::from_str(&s).unwrap())
        .collect();

    assert_eq!(events.len(), 2); // created + claimed
    assert_eq!(events[1]["action"], "claimed");
    assert_eq!(events[1]["actor"], "alice");
    assert_eq!(events[1]["taskNumber"], 1);
    assert_eq!(events[1]["prevStatus"], "todo");
    assert_eq!(events[1]["nextStatus"], "in_progress");
    assert_eq!(events[1]["claimedBy"], "alice");
}

#[test]
fn unclaim_task_emits_unclaimed_event() {
    let (store, _dir) = make_store();
    store
        .create_channel(
            "eng",
            None,
            chorus::store::channels::ChannelType::Channel,
            None,
        )
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "eng", "alice", "human");
    store
        .create_tasks("eng", "alice", SenderType::Human, &["t"])
        .map(|(tasks, _)| tasks)
        .unwrap();
    store
        .update_tasks_claim("eng", "alice", SenderType::Human, &[1])
        .map(|(results, _)| results)
        .unwrap();

    store
        .update_task_unclaim("eng", "alice", SenderType::Human, 1)
        .unwrap();

    let last_event: serde_json::Value = {
        let payload: String = store
            .conn_for_test()
            .query_row(
                "SELECT payload FROM messages WHERE sender_type = 'system' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&payload).unwrap()
    };
    assert_eq!(last_event["action"], "unclaimed");
    assert_eq!(last_event["prevStatus"], "in_progress");
    assert_eq!(last_event["nextStatus"], "todo");
    assert_eq!(last_event["claimedBy"], serde_json::Value::Null);
}

#[test]
fn update_task_status_emits_status_changed_event() {
    let (store, _dir) = make_store();
    store
        .create_channel(
            "eng",
            None,
            chorus::store::channels::ChannelType::Channel,
            None,
        )
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "eng", "alice", "human");
    store
        .create_tasks("eng", "alice", SenderType::Human, &["t"])
        .map(|(tasks, _)| tasks)
        .unwrap();
    store
        .update_tasks_claim("eng", "alice", SenderType::Human, &[1])
        .map(|(results, _)| results)
        .unwrap();

    store
        .update_task_status("eng", 1, "alice", SenderType::Human, TaskStatus::InReview)
        .unwrap();

    let last_event: serde_json::Value = {
        let payload: String = store
            .conn_for_test()
            .query_row(
                "SELECT payload FROM messages WHERE sender_type = 'system' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&payload).unwrap()
    };
    assert_eq!(last_event["action"], "status_changed");
    assert_eq!(last_event["actor"], "alice");
    assert_eq!(last_event["prevStatus"], "in_progress");
    assert_eq!(last_event["nextStatus"], "in_review");
}

#[test]
fn get_unread_summary_excludes_archived_task_sub_channels() {
    // Archived task sub-channels are terminal work, hidden from the active UI.
    // The agent start-prompt builder reads `get_unread_summary`; without this
    // filter an agent resuming after a task hits `Done` gets told it has
    // unread messages in a channel it can't navigate to. Mirrors the filter
    // used by `get_inbox_conversation_notifications` for the sidebar.
    let (store, _dir) = make_store();
    let sub_id = store
        .create_channel("eng__task-1", None, ChannelType::Task, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    store.ensure_human_with_id("bob", "bob").unwrap();
    store
        .join_channel_by_id(&sub_id, "alice", SenderType::Human)
        .map(|(joined, _)| joined)
        .unwrap();
    store
        .join_channel_by_id(&sub_id, "bob", SenderType::Human)
        .map(|(joined, _)| joined)
        .unwrap();
    // Bob's non-system message is unread for alice (the view excludes system
    // messages, so only human/agent traffic can produce a leak).
    store
        .create_message(CreateMessage {
            channel_name: "eng__task-1",
            sender_id: "bob",
            sender_type: SenderType::Human,
            content: "hi alice",
            attachment_ids: &[],
            suppress_event: true,
            run_id: None,
        })
        .map(|(id, _)| id)
        .unwrap();

    let before = store.get_unread_summary("alice").unwrap();
    assert_eq!(
        before.get("eng__task-1"),
        Some(&1),
        "pre-archive: unread summary must include the sub-channel"
    );

    // Simulate the `Done` transition's archive step (direct UPDATE; the full
    // lifecycle path is covered by `task_lifecycle_emits_four_events_in_parent_channel`).
    store
        .conn_for_test()
        .execute(
            "UPDATE channels SET archived = 1 WHERE id = ?1",
            params![sub_id],
        )
        .unwrap();

    let after = store.get_unread_summary("alice").unwrap();
    assert!(
        !after.contains_key("eng__task-1"),
        "post-archive: unread summary must exclude archived task sub-channels, \
         got {:?}",
        after
    );
}
