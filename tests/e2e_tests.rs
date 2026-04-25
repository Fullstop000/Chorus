mod harness;

use std::sync::Arc;

use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, SenderType};
use chorus::store::AgentRecordUpsert;
use chorus::store::Store;
use harness::build_router;

async fn start_test_server() -> (String, Arc<Store>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.create_human("testuser").unwrap();
    store
        .create_channel("general", Some("General"), ChannelType::Channel, None)
        .unwrap();
    store
        .join_channel("general", "testuser", SenderType::Human)
        .unwrap();
    let router = build_router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (url, store)
}

/// Test 1: Human sends message, agent receives via HTTP
#[tokio::test]
async fn test_human_to_agent_message_flow() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

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
            sender_name: "testuser",
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
            sender_name: "testuser",
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

    // Claim task 1 — sets owner; status stays todo (decoupled from start).
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

    // Walk forward step-by-step: todo -> in_progress -> in_review.
    let resp = client
        .post(format!(
            "{url}/api/conversations/{channel_id}/tasks/update-status"
        ))
        .json(&serde_json::json!({
            "task_number": 1,
            "status": "in_progress"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

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
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

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

    store
        .create_agent_record(&AgentRecordUpsert {
            name: "claude_bot",
            display_name: "Claude",
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
            name: "codex_bot",
            display_name: "Codex",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "o3",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "claude_bot", SenderType::Agent)
        .unwrap();
    store
        .join_channel("general", "codex_bot", SenderType::Agent)
        .unwrap();

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
    store.create_human("alice").unwrap();
    store
        .create_tasks("general", "alice", &["Ship it"])
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
    assert_eq!(tasks[0]["createdBy"], "alice");
}

/// Task 5 — `GET /api/conversations/{id}/tasks/{n}` returns a single task with
/// the full `TaskInfo` payload, including its sub-channel fields.
#[tokio::test]
async fn get_task_detail_returns_task_and_sub_channel() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store.create_human("alice").unwrap();
    store
        .create_tasks("general", "alice", &["Ship it"])
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
    assert_eq!(body["createdBy"], "alice");

    // Unknown task number → 404, not 500.
    let resp = client
        .get(format!("{url}/api/conversations/{channel_id}/tasks/999"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

/// Task lifecycle in the unified model:
/// - Parent channel: ONE `task_card` system message (the host) on creation.
///   Subsequent state changes do not post more parent-channel messages — the
///   card re-renders via SSE `task_update` instead.
/// - Sub-channel: kickoff + per-action `task_event` messages.
/// Walks create → claim → in_progress → in_review → done step-by-step
/// system messages in the parent channel.
#[tokio::test]
async fn task_lifecycle_emits_four_events_in_parent_channel() {
    use chorus::store::tasks::TaskStatus;

    let (_url, store) = start_test_server().await;
    let parent_id = store
        .create_channel("eng", None, ChannelType::Channel, None)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("eng", "alice", SenderType::Human)
        .unwrap();

    // create → claim → in_progress → in_review → done. Forward-only graph
    // means every step is explicit; claim no longer auto-advances to
    // in_progress.
    let created = store.create_tasks("eng", "alice", &["wire up"]).unwrap();
    let sub_id = created[0]
        .sub_channel_id
        .as_deref()
        .expect("sub-channel minted on create")
        .to_string();
    store.update_tasks_claim("eng", "alice", &[1]).unwrap();
    store
        .update_task_status("eng", 1, "alice", TaskStatus::InProgress)
        .unwrap();
    store
        .update_task_status("eng", 1, "alice", TaskStatus::InReview)
        .unwrap();
    store
        .update_task_status("eng", 1, "alice", TaskStatus::Done)
        .unwrap();

    // Parent channel: exactly ONE system message — the `task_card` host. No
    // task_event fires in the parent under the unified model; the card
    // re-renders via SSE `task_update` instead.
    let parent_msgs: Vec<serde_json::Value> = store
        .conn_for_test()
        .prepare(
            "SELECT content FROM messages \
             WHERE channel_id = ?1 AND sender_type = 'system' \
             ORDER BY seq",
        )
        .unwrap()
        .query_map(rusqlite::params![parent_id], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null))
        .collect();
    assert_eq!(
        parent_msgs.len(),
        1,
        "parent channel: exactly one task_card host message"
    );
    assert_eq!(parent_msgs[0]["kind"], "task_card");
    assert_eq!(parent_msgs[0]["status"], "todo");

    // Sub-channel: kickoff (plain text) + 4 task_events (claimed,
    // in_progress, in_review, done).
    let sub_rows: Vec<String> = store
        .conn_for_test()
        .prepare(
            "SELECT content FROM messages \
             WHERE channel_id = ?1 AND sender_type = 'system' \
             ORDER BY seq",
        )
        .unwrap()
        .query_map(rusqlite::params![sub_id], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(sub_rows.len(), 5, "kickoff + 4 task_events: {sub_rows:?}");
    assert!(
        sub_rows[0].starts_with("Task opened: wire up"),
        "first sub-channel message is the kickoff"
    );
    let events: Vec<serde_json::Value> = sub_rows
        .iter()
        .skip(1)
        .map(|c| serde_json::from_str(c).unwrap())
        .collect();
    assert_eq!(events[0]["action"], "claimed");
    assert_eq!(events[1]["action"], "status_changed");
    assert_eq!(events[1]["nextStatus"], "in_progress");
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
    store.create_human("bob").unwrap();
    store
        .join_channel("eng2", "bob", SenderType::Human)
        .unwrap();

    // Three tasks in one call exercises the per-task task_card emission. Each
    // creation posts ONE `task_card` host message in the parent channel with
    // the task's number/title/status; no per-task `created` task_event is
    // emitted in the unified model (the card replaces it).
    let created = store.create_tasks("eng2", "bob", &["a", "b", "c"]).unwrap();
    assert_eq!(created.len(), 3);

    let cards: Vec<serde_json::Value> = store
        .conn_for_test()
        .prepare(
            "SELECT content FROM messages \
             WHERE channel_id = ?1 AND sender_type = 'system' \
             ORDER BY seq",
        )
        .unwrap()
        .query_map(rusqlite::params![parent_id], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|s| serde_json::from_str(&s).unwrap())
        .collect();

    assert_eq!(cards.len(), 3, "one task_card per task");
    for (i, card) in cards.iter().enumerate() {
        assert_eq!(card["kind"], "task_card");
        assert_eq!(card["taskNumber"], (i + 1) as i64);
        assert_eq!(card["createdBy"], "bob");
        assert_eq!(card["status"], "todo");
    }
    assert_eq!(cards[0]["title"], "a");
    assert_eq!(cards[1]["title"], "b");
    assert_eq!(cards[2]["title"], "c");
}

/// HTTP-level pointer-vs-truth: agent proposes a task tied to a chat
/// message, the source message is deleted, and the task detail endpoint
/// still returns the snapshot. Provenance survives source deletion when
/// observed through the public API, not just at the SQL layer.
#[tokio::test]
async fn http_source_message_delete_preserves_task_snapshot() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot",
            display_name: "Bot",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store.create_human("alice").unwrap();

    let src_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "broken on safari mobile",
            attachment_ids: &[],
            suppress_event: true,
            run_id: None,
        })
        .unwrap();

    // Agent proposes via the bridge HTTP route. Agent routes are nested
    // under /internal/.
    let raw = client
        .post(format!("{url}/internal/agent/bot/tasks/propose"))
        .json(&serde_json::json!({
            "channel": "general",
            "title": "fix safari",
            "source_message_id": src_id,
        }))
        .send()
        .await
        .unwrap();
    assert!(
        raw.status().is_success(),
        "propose failed: {} body={:?}",
        raw.status(),
        raw.text().await.unwrap_or_default()
    );
    let resp: serde_json::Value = client
        .post(format!("{url}/internal/agent/bot/tasks/propose"))
        .json(&serde_json::json!({
            "channel": "general",
            "title": "fix safari take 2",
            "source_message_id": src_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let task_number = resp["taskNumber"].as_i64().expect("taskNumber on response");

    // Delete the source message.
    store
        .conn_for_test()
        .execute(
            "DELETE FROM messages WHERE id = ?1",
            rusqlite::params![src_id],
        )
        .unwrap();

    // The public task-detail endpoint still serves the snapshot fields,
    // with sourceMessageId nulled out.
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    let body: serde_json::Value = client
        .get(format!(
            "{url}/api/conversations/{channel_id}/tasks/{task_number}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(
        body["sourceMessageId"].is_null(),
        "sourceMessageId must NULL after source delete, got {body:?}"
    );
    assert_eq!(
        body["snapshotSenderName"], "alice",
        "snapshot sender preserved after source delete"
    );
    assert_eq!(
        body["snapshotContent"], "broken on safari mobile",
        "snapshot content preserved after source delete"
    );
}
