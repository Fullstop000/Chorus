mod harness;

use std::sync::Arc;

use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, SenderType};
use chorus::store::task_proposals::CreateTaskProposalInput;
use chorus::store::AgentRecordUpsert;
use chorus::store::Store;
use harness::build_router;

/// Seed a human-authored message into `channel_name` so v2
/// `create_task_proposal` has a concrete `source_message_id` to snapshot.
/// Idempotent on human-create / join so callers can reuse one sender across
/// several calls without extra bookkeeping.
fn seed_source_message(store: &Store, channel_name: &str, sender: &str, content: &str) -> String {
    let _ = store.create_human(sender);
    let _ = store.join_channel(channel_name, sender, SenderType::Human);
    store
        .create_message(CreateMessage {
            channel_name,
            sender_name: sender,
            sender_type: SenderType::Human,
            content,
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap()
}

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
    assert_eq!(tasks[0]["createdByName"], "alice");
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
    store.create_human("alice").unwrap();
    store
        .join_channel("eng", "alice", SenderType::Human)
        .unwrap();

    // create → claim → in_review → done
    store.create_tasks("eng", "alice", &["wire up"]).unwrap();
    store.update_tasks_claim("eng", "alice", &[1]).unwrap();
    store
        .update_task_status("eng", 1, "alice", TaskStatus::InReview)
        .unwrap();
    store
        .update_task_status("eng", 1, "alice", TaskStatus::Done)
        .unwrap();

    let events: Vec<serde_json::Value> = store
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
    store.create_human("bob").unwrap();
    store
        .join_channel("eng2", "bob", SenderType::Human)
        .unwrap();

    // Three tasks in one call exercises the pending_events Vec with > 1 entry.
    let created = store.create_tasks("eng2", "bob", &["a", "b", "c"]).unwrap();
    assert_eq!(created.len(), 3);

    let events: Vec<serde_json::Value> = store
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

#[tokio::test]
async fn http_accept_task_proposal_returns_task_coords() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    let channel_id = store
        .create_channel("eng", None, ChannelType::Channel, None)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "claude",
            display_name: "Claude",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    let msg_id = seed_source_message(&store, "eng", "alice", "hi");
    let p = store
        .create_task_proposal(CreateTaskProposalInput {
            channel_id: &channel_id,
            proposed_by: "claude",
            title: "fix login",
            source_message_id: &msg_id,
        })
        .unwrap();

    let resp = client
        .post(format!("{url}/api/task-proposals/{}/accept", p.id))
        .json(&serde_json::json!({ "accepter": "alice" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["taskNumber"], 1);
    assert!(body["subChannelId"].is_string());
    assert!(body["subChannelName"]
        .as_str()
        .unwrap()
        .ends_with("__task-1"));
}

#[tokio::test]
async fn http_dismiss_task_proposal_returns_204() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();
    let channel_id = store
        .create_channel("eng", None, ChannelType::Channel, None)
        .unwrap();
    let msg_id = seed_source_message(&store, "eng", "alice", "hi");
    let p = store
        .create_task_proposal(CreateTaskProposalInput {
            channel_id: &channel_id,
            proposed_by: "claude",
            title: "t",
            source_message_id: &msg_id,
        })
        .unwrap();

    let resp = client
        .post(format!("{url}/api/task-proposals/{}/dismiss", p.id))
        .json(&serde_json::json!({ "resolver": "alice" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
}

#[tokio::test]
async fn http_get_task_proposal_returns_current_state() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();
    let channel_id = store
        .create_channel("eng", None, ChannelType::Channel, None)
        .unwrap();
    let msg_id = seed_source_message(&store, "eng", "alice", "hi");
    let p = store
        .create_task_proposal(CreateTaskProposalInput {
            channel_id: &channel_id,
            proposed_by: "claude",
            title: "t",
            source_message_id: &msg_id,
        })
        .unwrap();

    let resp = client
        .get(format!("{url}/api/task-proposals/{}", p.id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], p.id);
    assert_eq!(body["status"], "pending");
    assert_eq!(body["title"], "t");
}

#[tokio::test]
async fn http_accept_nonexistent_proposal_returns_404() {
    let (url, _store) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{url}/api/task-proposals/does-not-exist/accept"))
        .json(&serde_json::json!({ "accepter": "alice" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn http_dismiss_already_dismissed_returns_409() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();
    let channel_id = store
        .create_channel("eng", None, ChannelType::Channel, None)
        .unwrap();
    let msg_id = seed_source_message(&store, "eng", "alice", "hi");
    let p = store
        .create_task_proposal(CreateTaskProposalInput {
            channel_id: &channel_id,
            proposed_by: "claude",
            title: "t",
            source_message_id: &msg_id,
        })
        .unwrap();

    // First dismiss succeeds.
    let first = client
        .post(format!("{url}/api/task-proposals/{}/dismiss", p.id))
        .json(&serde_json::json!({ "resolver": "alice" }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 204);

    // Second dismiss — proposal already resolved. Expect 409 + the
    // machine-readable code `TASK_PROPOSAL_ALREADY_RESOLVED`.
    let second = client
        .post(format!("{url}/api/task-proposals/{}/dismiss", p.id))
        .json(&serde_json::json!({ "resolver": "alice" }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 409);
    let body: serde_json::Value = second.json().await.unwrap();
    assert_eq!(body["code"], "TASK_PROPOSAL_ALREADY_RESOLVED");
}

#[tokio::test]
async fn internal_agent_create_proposal_inserts_row_and_card() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();
    store
        .create_channel("eng", None, ChannelType::Channel, None)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "claude",
            display_name: "Claude",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    // v2: the internal endpoint now requires sourceMessageId (the user
    // request the agent is proposing against), so seed one.
    let msg_id = seed_source_message(&store, "eng", "alice", "please fix login");

    let resp = client
        .post(format!(
            "{url}/internal/agent/claude/channels/eng/task-proposals"
        ))
        .json(&serde_json::json!({
            "title": "investigate login 500",
            "sourceMessageId": msg_id,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["id"].is_string());
    assert_eq!(body["status"], "pending");
    assert_eq!(body["title"], "investigate login 500");
    assert_eq!(body["proposedBy"], "claude");
}
