use std::sync::Arc;

use chorus::server::build_router;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, SenderType};
use chorus::store::Store;

async fn start_test_server() -> (String, Arc<Store>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.create_human("testuser").unwrap();
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();

    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "testuser",
            sender_type: SenderType::Human,
            content: "hello bot",
            attachment_ids: &[],
            suppress_event: false,
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
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
            thread_parent_id: None,
            sender_name: "testuser",
            sender_type: SenderType::Human,
            content: "wake up!",
            attachment_ids: &[],
            suppress_event: false,
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
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
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
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
        .create_agent_record("claude_bot", "Claude", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .create_agent_record("codex_bot", "Codex", None, "codex", "o3", &[])
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

/// Test 7: Team thread targets round-trip correctly for Codex agents
#[tokio::test]
async fn test_team_thread_target_round_trip_for_codex_agent() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    let team_id = store
        .create_team("qa-eng", "QA Engineering", "leader_operators", Some("bot1"))
        .unwrap();
    store
        .create_channel("qa-eng", Some("QA Engineering"), ChannelType::Team)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "codex", "gpt-5.4-mini", &[])
        .unwrap();
    store
        .create_team_member(&team_id, "bot1", "agent", "bot1", "leader")
        .unwrap();
    store
        .create_team_member(&team_id, "testuser", "human", "testuser", "observer")
        .unwrap();
    store
        .join_channel("qa-eng", "bot1", SenderType::Agent)
        .unwrap();
    store
        .join_channel("qa-eng", "testuser", SenderType::Human)
        .unwrap();

    let parent_resp: serde_json::Value = client
        .post(format!("{url}/internal/agent/bot1/send"))
        .json(&serde_json::json!({
            "target": "#qa-eng",
            "content": "bot1 team parent"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let parent_id = parent_resp["messageId"].as_str().unwrap();
    let short_id = &parent_id[..8];
    let thread_target = format!("#qa-eng:{short_id}");

    let send_resp = client
        .post(format!("{url}/internal/agent/testuser/send"))
        .json(&serde_json::json!({
            "target": thread_target,
            "content": "please stay in the team thread"
        }))
        .send()
        .await
        .unwrap();
    assert!(send_resp.status().is_success());

    let thread_history: serde_json::Value = client
        .get(format!(
            "{url}/internal/agent/testuser/history?channel={}&limit=10",
            urlencoding::encode(&format!("#qa-eng:{short_id}"))
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let thread_messages = thread_history["messages"].as_array().unwrap();
    assert_eq!(thread_messages.len(), 1);
    assert_eq!(
        thread_messages[0]["content"].as_str().unwrap(),
        "please stay in the team thread"
    );
    assert_eq!(
        thread_messages[0]["senderName"].as_str().unwrap(),
        "testuser"
    );

    let top_level_history: serde_json::Value = client
        .get(format!(
            "{url}/internal/agent/testuser/history?channel=%23qa-eng&limit=10"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let top_level_messages = top_level_history["messages"].as_array().unwrap();
    assert_eq!(top_level_messages.len(), 1);
    assert_eq!(
        top_level_messages[0]["content"].as_str().unwrap(),
        "bot1 team parent"
    );

    let receive_resp: serde_json::Value = client
        .get(format!("{url}/internal/agent/bot1/receive?block=false"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let received_messages = receive_resp["messages"].as_array().unwrap();
    assert!(received_messages.iter().any(|message| {
        message["channel_type"].as_str() == Some("thread")
            && message["parent_channel_name"].as_str() == Some("qa-eng")
            && message["content"].as_str() == Some("please stay in the team thread")
    }));
}
