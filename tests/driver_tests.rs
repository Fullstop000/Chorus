use chorus::agent::config::AgentConfig;
use chorus::agent::drivers::claude::ClaudeDriver;
use chorus::agent::drivers::codex::CodexDriver;
use chorus::agent::drivers::kimi::KimiDriver;
use chorus::agent::drivers::Driver;

#[test]
fn test_claude_prompt_uses_split_message_tools() {
    let driver = ClaudeDriver;
    let config = AgentConfig {
        name: "claude-bot".to_string(),
        display_name: "Claude Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        runtime: "claude".to_string(),
        model: "sonnet".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
        teams: vec![],
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        prompt.contains("mcp__chat__wait_for_message"),
        "Claude prompts must teach the blocking idle tool explicitly"
    );
    assert!(
        prompt.contains("mcp__chat__check_messages"),
        "Claude prompts must teach the non-blocking message check explicitly"
    );
    assert!(
        !prompt.contains("mcp__chat__receive_message"),
        "Claude prompts should not rely on the legacy combined receive tool"
    );
}

#[test]
fn test_codex_prompt_uses_split_message_tools() {
    let driver = CodexDriver;
    let config = AgentConfig {
        name: "codex-bot".to_string(),
        display_name: "Codex Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        runtime: "codex".to_string(),
        model: "gpt-5.4-mini".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
        teams: vec![],
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        prompt.contains("mcp_chat_wait_for_message"),
        "Codex prompts must reference the blocking idle tool"
    );
    assert!(
        prompt.contains("mcp_chat_check_messages"),
        "Codex prompts must reference the non-blocking check tool"
    );
    assert!(
        prompt.contains("mcp_chat_send_message"),
        "Codex prompts must reference the actual MCP send tool"
    );
    assert!(
        !prompt.contains("mcp_chat_receive_message"),
        "Codex prompts should not rely on the legacy combined receive tool"
    );
    assert!(
        prompt.contains("mcp_chat_view_file"),
        "Codex prompts should teach attachment inspection explicitly"
    );
    assert!(
        prompt.contains("Chorus"),
        "Codex prompts should use the current product name"
    );
}

#[test]
fn test_kimi_prompt_uses_split_message_tools() {
    let driver = KimiDriver;
    let config = AgentConfig {
        name: "kimi-bot".to_string(),
        display_name: "Kimi Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        runtime: "kimi".to_string(),
        model: "kimi-code/kimi-for-coding".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
        teams: vec![],
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        prompt.contains("wait_for_message"),
        "Kimi prompts must teach the blocking idle tool explicitly with the Kimi-native tool name"
    );
    assert!(
        prompt.contains("check_messages"),
        "Kimi prompts must teach the non-blocking message check explicitly with the Kimi-native tool name"
    );
    assert!(
        !prompt.contains("receive_message"),
        "Kimi prompts should not rely on the legacy combined receive tool"
    );
    assert!(
        prompt.contains("view_file"),
        "Kimi prompts should teach attachment inspection explicitly"
    );
    assert!(
        prompt.contains("Chorus"),
        "Kimi prompts should use the current product name"
    );
    assert!(
        prompt.contains("must either send a reply or deliberately explain why no reply is needed"),
        "Kimi prompts should explicitly forbid consuming a message and silently returning to idle"
    );
    assert!(
        prompt.contains("Any reply meant for humans must be delivered with `send_message()`"),
        "Kimi prompts should explicitly forbid treating raw stdout text as a valid human reply"
    );
}
