use chorus::agent::config::AgentConfig;
use chorus::agent::drivers::acp::AcpDriver;
use chorus::agent::drivers::claude::ClaudeAcpRuntime;
use chorus::agent::drivers::claude_raw::ClaudeRawDriver;
use chorus::agent::drivers::codex::CodexAcpRuntime;
use chorus::agent::drivers::codex_raw::CodexRawDriver;
use chorus::agent::drivers::kimi::KimiAcpRuntime;
use chorus::agent::drivers::kimi_raw::KimiRawDriver;
use chorus::agent::drivers::opencode::OpencodeAcpRuntime;
use chorus::agent::drivers::opencode_raw::OpencodeRawDriver;
use chorus::agent::drivers::Driver;

// ── Raw (1.0) driver prompt tests ──

#[test]
fn test_claude_raw_prompt_uses_split_message_tools() {
    let driver = ClaudeRawDriver;
    let config = AgentConfig {
        name: "claude-bot".to_string(),
        display_name: "Claude Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        system_prompt: None,
        runtime: "claude".to_string(),
        model: "sonnet".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        !prompt.contains("mcp__chat__wait_for_message"),
        "push-idle: agents no longer use wait_for_message"
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
fn test_codex_raw_prompt_uses_split_message_tools() {
    let driver = CodexRawDriver;
    let config = AgentConfig {
        name: "codex-bot".to_string(),
        display_name: "Codex Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        system_prompt: None,
        runtime: "codex".to_string(),
        model: "gpt-5.4-mini".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        !prompt.contains("mcp_chat_wait_for_message"),
        "push-idle: agents no longer use wait_for_message"
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
fn test_kimi_raw_prompt_uses_split_message_tools() {
    let driver = KimiRawDriver;
    let config = AgentConfig {
        name: "kimi-bot".to_string(),
        display_name: "Kimi Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        system_prompt: None,
        runtime: "kimi".to_string(),
        model: "kimi-code/kimi-for-coding".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        !prompt.contains("wait_for_message"),
        "push-idle: agents no longer use wait_for_message"
    );
    assert!(
        prompt.contains("check_messages"),
        "Kimi prompts must teach the non-blocking message check explicitly"
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
}

#[test]
fn test_opencode_raw_prompt_uses_split_message_tools() {
    let driver = OpencodeRawDriver;
    let config = AgentConfig {
        name: "opencode-bot".to_string(),
        display_name: "OpenCode Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        system_prompt: None,
        runtime: "opencode".to_string(),
        model: "anthropic/claude-sonnet-4-20250514".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        !prompt.contains("chat_wait_for_message"),
        "push-idle: agents no longer use wait_for_message"
    );
    assert!(
        prompt.contains("chat_check_messages"),
        "OpenCode prompts must reference the non-blocking check tool"
    );
    assert!(
        prompt.contains("chat_send_message"),
        "OpenCode prompts must reference the actual MCP send tool"
    );
    assert!(
        !prompt.contains("chat_receive_message"),
        "OpenCode prompts should not rely on the legacy combined receive tool"
    );
    assert!(
        prompt.contains("chat_view_file"),
        "OpenCode prompts should teach attachment inspection explicitly"
    );
    assert!(
        prompt.contains("Chorus"),
        "OpenCode prompts should use the current product name"
    );
}

// ── ACP (2.0) driver prompt tests ──

#[test]
fn test_claude_acp_prompt_uses_split_message_tools() {
    let driver = AcpDriver::new(ClaudeAcpRuntime);
    let config = AgentConfig {
        name: "claude-bot".to_string(),
        display_name: "Claude Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        system_prompt: None,
        runtime: "claude".to_string(),
        model: "sonnet".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        !prompt.contains("mcp__chat__wait_for_message"),
        "push-idle: agents no longer use wait_for_message"
    );
    assert!(
        prompt.contains("mcp__chat__check_messages"),
        "ACP Claude prompts must teach the non-blocking message check"
    );
}

#[test]
fn test_codex_acp_prompt_uses_split_message_tools() {
    let driver = AcpDriver::new(CodexAcpRuntime);
    let config = AgentConfig {
        name: "codex-bot".to_string(),
        display_name: "Codex Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        system_prompt: None,
        runtime: "codex".to_string(),
        model: "gpt-5.4-mini".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        prompt.contains("mcp_chat_check_messages"),
        "ACP Codex prompts must reference the non-blocking check tool"
    );
    assert!(
        prompt.contains("mcp_chat_send_message"),
        "ACP Codex prompts must reference the actual MCP send tool"
    );
}

#[test]
fn test_kimi_acp_prompt_uses_split_message_tools() {
    let driver = AcpDriver::new(KimiAcpRuntime);
    let config = AgentConfig {
        name: "kimi-bot".to_string(),
        display_name: "Kimi Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        system_prompt: None,
        runtime: "kimi".to_string(),
        model: "kimi-code/kimi-for-coding".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        prompt.contains("check_messages"),
        "ACP Kimi prompts must teach the non-blocking message check"
    );
}

#[test]
fn test_opencode_acp_prompt_uses_split_message_tools() {
    let driver = AcpDriver::new(OpencodeAcpRuntime);
    let config = AgentConfig {
        name: "opencode-bot".to_string(),
        display_name: "OpenCode Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        system_prompt: None,
        runtime: "opencode".to_string(),
        model: "anthropic/claude-sonnet-4-20250514".to_string(),
        session_id: None,
        reasoning_effort: None,
        env_vars: Vec::new(),
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        prompt.contains("chat_check_messages"),
        "ACP OpenCode prompts must reference the non-blocking check tool"
    );
    assert!(
        prompt.contains("chat_send_message"),
        "ACP OpenCode prompts must reference the actual MCP send tool"
    );
}
