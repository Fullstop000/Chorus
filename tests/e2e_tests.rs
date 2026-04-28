mod harness;

use std::sync::Arc;

use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, SenderType};
use chorus::store::Store;
use harness::{build_router, join_channel_silent};

async fn start_test_server() -> (String, Arc<Store>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    // Pre-create `#all` so the migration in `ensure_all_channel_inner`
    // (which renames `#general` -> `#all` when no `#all` exists)
    // does not fire when `build_router` calls `ensure_builtin_channels`.
    store
        .create_channel(
            Store::DEFAULT_SYSTEM_CHANNEL,
            None,
            ChannelType::System,
            None,
        )
        .unwrap();
    store.ensure_human_with_id("testuser", "testuser").unwrap();
    store
        .create_channel("general", Some("General"), ChannelType::Channel, None)
        .unwrap();
    join_channel_silent(&store, "general", "testuser", "human");
    let router = build_router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (url, store)
}

/// Insert an agent row with a chosen primary key. Used by e2e
/// fixtures so `/internal/agent/{agent_id}` URLs and `"bot1"`-style
/// identity-typed args keep working under the strict ID-first store
/// without having to thread a UUID through every test body.
fn seed_agent_with_id(
    store: &Arc<Store>,
    id: &str,
    display_name: &str,
    runtime: &str,
    model: &str,
) {
    let workspace_id = store
        .get_active_workspace()
        .unwrap()
        .expect("seed_agent_with_id requires an active workspace")
        .id;
    let conn = store.conn_for_test();
    conn.execute(
        "INSERT INTO agents (id, workspace_id, name, display_name, runtime, model)
         VALUES (?1, ?2, ?1, ?3, ?4, ?5)",
        rusqlite::params![id, workspace_id, display_name, runtime, model],
    )
    .unwrap();
}

/// Test 1: Human sends message, agent receives via HTTP
#[tokio::test]
async fn test_human_to_agent_message_flow() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    seed_agent_with_id(&store, "bot1", "Bot 1", "claude", "sonnet");
    join_channel_silent(&store, "general", "bot1", "agent");

    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "testuser",
            sender_type: SenderType::Human,
            content: "hello bot",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let resp: serde_json::Value = client
        .get(format!("{url}/internal/agent/bot1/receive?block=false"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let messages = resp["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"].as_str().unwrap(), "hello bot");
    assert_eq!(messages[0]["sender_name"].as_str().unwrap(), "testuser");
}

/// Test 2: Agent replies, appears in history
#[tokio::test]
async fn test_agent_reply_in_history() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    seed_agent_with_id(&store, "bot1", "Bot 1", "claude", "sonnet");
    join_channel_silent(&store, "general", "bot1", "agent");

    let resp = client
        .post(format!("{url}/internal/agent/bot1/send"))
        .json(&serde_json::json!({
            "target": "#general",
            "content": "hi humans!"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let resp: serde_json::Value = client
        .get(format!(
            "{url}/internal/agent/bot1/history?channel=%23general&limit=10"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let messages = resp["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["senderName"].as_str().unwrap(), "bot1");
    assert_eq!(messages[0]["content"].as_str().unwrap(), "hi humans!");
}

/// Test 3: Blocking receive wakes on message
#[tokio::test]
async fn test_blocking_receive_wakes_on_message() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    seed_agent_with_id(&store, "bot1", "Bot 1", "claude", "sonnet");
    join_channel_silent(&store, "general", "bot1", "agent");

    let url2 = url.clone();
    let client2 = client.clone();
    let recv_handle = tokio::spawn(async move {
        let resp: serde_json::Value = client2
            .get(format!(
                "{url2}/internal/agent/bot1/receive?block=true&timeout=5000"
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        resp
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "testuser",
            sender_type: SenderType::Human,
            content: "wake up!",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let resp = tokio::time::timeout(std::time::Duration::from_secs(3), recv_handle)
        .await
        .unwrap()
        .unwrap();

    let messages = resp["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"].as_str().unwrap(), "wake up!");
}

/// Test 4: Task board workflow (create, claim, update status, list)
#[tokio::test]
async fn test_task_board_e2e() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    seed_agent_with_id(&store, "bot1", "Bot 1", "claude", "sonnet");
    join_channel_silent(&store, "general", "bot1", "agent");

    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    // Create tasks
    let resp: serde_json::Value = client
        .post(format!("{url}/api/conversations/{channel_id}/tasks"))
        .json(&serde_json::json!({
            "tasks": [{"title": "Task A"}, {"title": "Task B"}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["tasks"].as_array().unwrap().len(), 2);

    // Claim task 1 (transitions from todo -> in_progress)
    let resp: serde_json::Value = client
        .post(format!("{url}/api/conversations/{channel_id}/tasks/claim"))
        .json(&serde_json::json!({
            "task_numbers": [1]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resp["results"][0]["success"].as_bool().unwrap());

    // Update status to in_review (in_progress -> in_review)
    let resp = client
        .post(format!(
            "{url}/api/conversations/{channel_id}/tasks/update-status"
        ))
        .json(&serde_json::json!({
            "task_number": 1,
            "status": "in_review"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // List tasks — task 1 should be in_review
    let resp: serde_json::Value = client
        .get(format!("{url}/api/conversations/{channel_id}/tasks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tasks = resp["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0]["status"].as_str().unwrap(), "in_review");
    assert_eq!(tasks[1]["status"].as_str().unwrap(), "todo");
}

/// Test 5: Workspace listing and file preview flow over HTTP
#[tokio::test]
async fn test_workspace_e2e_lists_and_reads_markdown_file() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    seed_agent_with_id(&store, "bot1", "Bot 1", "claude", "sonnet");
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    join_channel_silent(&store, "general", "bot1", "agent");

    let workspace_dir = store.agents_dir().join("bot1");
    let notes_dir = workspace_dir.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(
        workspace_dir.join("MEMORY.md"),
        "# Memory
",
    )
    .unwrap();
    std::fs::write(
        notes_dir.join("work-log.md"),
        "# Work Log

- first entry
",
    )
    .unwrap();

    let workspace: serde_json::Value = client
        .get(format!("{url}/api/agents/{}/workspace", bot1.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        workspace["path"].as_str(),
        Some(workspace_dir.to_string_lossy().as_ref())
    );
    let files = workspace["files"].as_array().unwrap();
    assert!(files.iter().any(|entry| entry == "notes/"));
    assert!(files.iter().any(|entry| entry == "notes/work-log.md"));
    assert!(files.iter().any(|entry| entry == "MEMORY.md"));

    let preview: serde_json::Value = client
        .get(format!(
            "{url}/api/agents/{}/workspace/file?path=notes%2Fwork-log.md",
            bot1.id
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(preview["path"].as_str(), Some("notes/work-log.md"));
    assert_eq!(
        preview["content"].as_str(),
        Some(
            "# Work Log

- first entry
"
        )
    );
    assert_eq!(preview["sizeBytes"].as_u64(), Some(26));
    assert_eq!(preview["truncated"].as_bool(), Some(false));
    assert!(preview["modifiedMs"].as_u64().is_some());
}

/// Test 6: Multi-agent communication
#[tokio::test]
async fn test_multi_agent_channel_communication() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    seed_agent_with_id(&store, "claude_bot", "Claude", "claude", "sonnet");
    seed_agent_with_id(&store, "codex_bot", "Codex", "codex", "o3");
    join_channel_silent(&store, "general", "claude_bot", "agent");
    join_channel_silent(&store, "general", "codex_bot", "agent");

    // Claude sends a message
    client
        .post(format!("{url}/internal/agent/claude_bot/send"))
        .json(&serde_json::json!({
            "target": "#general",
            "content": "I'll handle the architecture"
        }))
        .send()
        .await
        .unwrap();

    // Codex receives it
    let resp: serde_json::Value = client
        .get(format!(
            "{url}/internal/agent/codex_bot/receive?block=false"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let messages = resp["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["sender_name"].as_str().unwrap(), "claude_bot");

    // Human also sees it in history
    let resp: serde_json::Value = client
        .get(format!(
            "{url}/internal/agent/testuser/history?channel=%23general&limit=10"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["messages"].as_array().unwrap().len(), 1);
}

/// Task 5 — the task board list response carries the task's sub-channel info
/// so the UI can deep-link from each row into its child channel.
#[tokio::test]
async fn list_tasks_returns_sub_channel_info() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    // Create tasks directly in the store so the creator name is stable across
    // machines (the public endpoint stamps `whoami::username()`, which isn't
    // useful for an assertion).
    store.ensure_human_with_id("alice", "alice").unwrap();
    store
        .create_tasks("general", "alice", SenderType::Human, &["Ship it"])
        .unwrap();

    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    let resp: serde_json::Value = client
        .get(format!("{url}/api/conversations/{channel_id}/tasks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let tasks = resp["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["subChannelName"], "general__task-1");
    assert!(tasks[0]["subChannelId"].is_string());
    assert_eq!(tasks[0]["createdByName"], "alice");
}

/// Task 5 — `GET /api/conversations/{id}/tasks/{n}` returns a single task with
/// the full `TaskInfo` payload, including its sub-channel fields.
#[tokio::test]
async fn get_task_detail_returns_task_and_sub_channel() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store.ensure_human_with_id("alice", "alice").unwrap();
    store
        .create_tasks("general", "alice", SenderType::Human, &["Ship it"])
        .unwrap();

    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    let resp = client
        .get(format!("{url}/api/conversations/{channel_id}/tasks/1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["taskNumber"], 1);
    assert_eq!(body["title"], "Ship it");
    assert_eq!(body["subChannelName"], "general__task-1");
    assert!(body["subChannelId"].is_string());
    assert_eq!(body["createdByName"], "alice");

    // Unknown task number → 404, not 500.
    let resp = client
        .get(format!("{url}/api/conversations/{channel_id}/tasks/999"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

/// Task lifecycle: create → claim → in_review → done emits exactly 4 ordered
/// system messages in the parent channel.
#[tokio::test]
async fn task_lifecycle_emits_four_events_in_parent_channel() {
    use chorus::store::tasks::TaskStatus;

    let (_url, store) = start_test_server().await;
    let parent_id = store
        .create_channel("eng", None, ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(&store, "eng", "alice", "human");

    // create → claim → in_review → done
    store
        .create_tasks("eng", "alice", SenderType::Human, &["wire up"])
        .unwrap();
    store
        .update_tasks_claim("eng", "alice", SenderType::Human, &[1])
        .unwrap();
    store
        .update_task_status("eng", 1, "alice", SenderType::Human, TaskStatus::InReview)
        .unwrap();
    store
        .update_task_status("eng", 1, "alice", SenderType::Human, TaskStatus::Done)
        .unwrap();

    let events: Vec<serde_json::Value> = store
        .conn_for_test()
        .prepare(
            "SELECT payload FROM messages \
             WHERE channel_id = ?1 AND sender_type = 'system' \
             ORDER BY seq",
        )
        .unwrap()
        .query_map(rusqlite::params![parent_id], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|s| serde_json::from_str(&s).unwrap())
        .collect();

    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["action"], "created");
    assert_eq!(events[1]["action"], "claimed");
    assert_eq!(events[2]["action"], "status_changed");
    assert_eq!(events[2]["nextStatus"], "in_review");
    assert_eq!(events[3]["action"], "status_changed");
    assert_eq!(events[3]["nextStatus"], "done");
}

/// Batched create_tasks with 3 titles exercises the pending_events Vec with
/// more than one entry and emits one `created` event per task.
#[tokio::test]
async fn batched_create_tasks_emits_one_event_per_task() {
    let (_url, store) = start_test_server().await;
    let parent_id = store
        .create_channel("eng2", None, ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("bob", "bob").unwrap();
    join_channel_silent(&store, "eng2", "bob", "human");

    // Three tasks in one call exercises the pending_events Vec with > 1 entry.
    let created = store
        .create_tasks("eng2", "bob", SenderType::Human, &["a", "b", "c"])
        .unwrap();
    assert_eq!(created.len(), 3);

    let events: Vec<serde_json::Value> = store
        .conn_for_test()
        .prepare(
            "SELECT payload FROM messages \
             WHERE channel_id = ?1 AND sender_type = 'system' \
             ORDER BY seq",
        )
        .unwrap()
        .query_map(rusqlite::params![parent_id], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|s| serde_json::from_str(&s).unwrap())
        .collect();

    assert_eq!(events.len(), 3);
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event["action"], "created");
        assert_eq!(event["taskNumber"], (i + 1) as i64);
        assert_eq!(event["actor"], "bob");
    }
    // Task numbers are distinct 1, 2, 3 in creation order.
    assert_eq!(events[0]["title"], "a");
    assert_eq!(events[1]["title"], "b");
    assert_eq!(events[2]["title"], "c");
}
