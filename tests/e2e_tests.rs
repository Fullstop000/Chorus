use std::sync::Arc;

use chorus::models::{ChannelType, SenderType};
use chorus::server::build_router;
use chorus::store::Store;

async fn start_test_server() -> (String, Arc<Store>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.add_human("testuser").unwrap();
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet")
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    store
        .send_message(
            "general",
            None,
            "testuser",
            SenderType::Human,
            "hello bot",
            &[],
        )
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet")
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet")
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
        .send_message(
            "general",
            None,
            "testuser",
            SenderType::Human,
            "wake up!",
            &[],
        )
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet")
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    // Create tasks
    let resp: serde_json::Value = client
        .post(format!("{url}/internal/agent/bot1/tasks"))
        .json(&serde_json::json!({
            "channel": "#general",
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
        .post(format!("{url}/internal/agent/bot1/tasks/claim"))
        .json(&serde_json::json!({
            "channel": "#general",
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
        .post(format!("{url}/internal/agent/bot1/tasks/update-status"))
        .json(&serde_json::json!({
            "channel": "#general",
            "task_number": 1,
            "status": "in_review"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // List tasks — task 1 should be in_review
    let resp: serde_json::Value = client
        .get(format!(
            "{url}/internal/agent/bot1/tasks?channel=%23general"
        ))
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet")
        .unwrap();
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
        .get(format!("{url}/api/agents/bot1/workspace"))
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
            "{url}/api/agents/bot1/workspace/file?path=notes%2Fwork-log.md"
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

/// Test 6: DM and thread flow
#[tokio::test]
async fn test_dm_and_thread_flow() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet")
        .unwrap();

    // Agent sends DM to testuser
    let resp: serde_json::Value = client
        .post(format!("{url}/internal/agent/bot1/send"))
        .json(&serde_json::json!({
            "target": "dm:@testuser",
            "content": "hey!"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let msg_id = resp["messageId"].as_str().unwrap();
    let short_id = &msg_id[..8];

    // Reply in thread on that message
    let thread_target = format!("dm:@testuser:{short_id}");
    let resp = client
        .post(format!("{url}/internal/agent/bot1/send"))
        .json(&serde_json::json!({
            "target": thread_target,
            "content": "thread reply"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Read DM channel history (top-level messages only, thread reply is excluded)
    // The DM channel is named dm-bot1-testuser (names sorted alphabetically)
    let dm_channel_name = "dm-bot1-testuser";
    let resp: serde_json::Value = client
        .get(format!(
            "{url}/internal/agent/bot1/history?channel={}&limit=10",
            urlencoding::encode(dm_channel_name)
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let messages = resp["messages"].as_array().unwrap();
    // Only the parent message appears in top-level history (thread replies are excluded)
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"].as_str().unwrap(), "hey!");
    assert_eq!(messages[0]["senderName"].as_str().unwrap(), "bot1");
}

/// Test 6: Multi-agent communication
#[tokio::test]
async fn test_multi_agent_channel_communication() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store
        .create_agent_record("claude_bot", "Claude", None, "claude", "sonnet")
        .unwrap();
    store
        .create_agent_record("codex_bot", "Codex", None, "codex", "o3")
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
