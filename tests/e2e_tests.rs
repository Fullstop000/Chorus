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
