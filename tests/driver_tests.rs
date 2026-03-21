use chorus::drivers::codex::CodexDriver;
use chorus::drivers::Driver;
use chorus::models::AgentConfig;

#[test]
fn test_codex_prompt_uses_prefixed_chat_tools() {
    let driver = CodexDriver;
    let config = AgentConfig {
        name: "codex-bot".to_string(),
        display_name: "Codex Bot".to_string(),
        description: Some("Replies in Chorus".to_string()),
        runtime: "codex".to_string(),
        model: "gpt-5.4-mini".to_string(),
        session_id: None,
        env_vars: None,
    };

    let prompt = driver.build_system_prompt(&config, "agent-id");

    assert!(
        prompt.contains("mcp_chat_receive_message"),
        "Codex prompts must reference the actual MCP receive tool"
    );
    assert!(
        prompt.contains("mcp_chat_send_message"),
        "Codex prompts must reference the actual MCP send tool"
    );
    assert!(
        prompt.contains("block=false"),
        "Codex prompts should wake with a non-blocking receive flow"
    );
}
