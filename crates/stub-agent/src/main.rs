use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use regex::Regex;
use rmcp::model::CallToolRequestParams;
use rmcp::{ClientHandler, ServiceExt};
use serde::Deserialize;
use tokio::process::Command;

static SEQ: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// CLI args (minimal hand-parse to avoid adding clap)
// ---------------------------------------------------------------------------

struct Args {
    mcp_config: String,
    #[allow(dead_code)]
    prompt: String,
}

fn parse_args() -> Result<Args> {
    let args: Vec<String> = std::env::args().collect();
    let mut mcp_config = None;
    let mut prompt = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--mcp-config" => {
                i += 1;
                mcp_config = Some(args.get(i).context("missing --mcp-config value")?.clone());
            }
            "--prompt" => {
                i += 1;
                prompt = Some(args.get(i).context("missing --prompt value")?.clone());
            }
            _ => {}
        }
        i += 1;
    }
    Ok(Args {
        mcp_config: mcp_config.context("--mcp-config is required")?,
        prompt: prompt.context("--prompt is required")?,
    })
}

// ---------------------------------------------------------------------------
// MCP config parsing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct McpConfig {
    #[serde(rename = "mcpServers")]
    mcp_servers: std::collections::HashMap<String, McpServerEntry>,
}

#[derive(Deserialize)]
struct McpServerEntry {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

fn load_mcp_config(path: &str) -> Result<(String, Vec<String>)> {
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read MCP config at {path}"))?;
    let config: McpConfig =
        serde_json::from_str(&data).context("Failed to parse MCP config JSON")?;
    let entry = config
        .mcp_servers
        .get("chat")
        .context("No MCP server entry named 'chat' in config")?;
    Ok((entry.command.clone(), entry.args.clone()))
}

// ---------------------------------------------------------------------------
// JSON stdout protocol
// ---------------------------------------------------------------------------

fn emit(value: serde_json::Value) {
    // Print to our own stdout — the manager reads these lines.
    println!("{}", serde_json::to_string(&value).unwrap());
}

fn emit_session_init(session_id: &str) {
    emit(serde_json::json!({"type": "session_init", "session_id": session_id}));
}

fn emit_text(text: &str) {
    emit(serde_json::json!({"type": "text", "text": text}));
}

fn emit_tool_call(name: &str, input: &serde_json::Value) {
    emit(serde_json::json!({"type": "tool_call", "name": name, "input": input}));
}

fn emit_turn_end() {
    emit(serde_json::json!({"type": "turn_end"}));
}

fn emit_error(message: &str) {
    emit(serde_json::json!({"type": "error", "message": message}));
}

// ---------------------------------------------------------------------------
// MCP client handler (no-op — we only call tools, never receive requests)
// ---------------------------------------------------------------------------

struct StubClientHandler;
impl ClientHandler for StubClientHandler {}

// ---------------------------------------------------------------------------
// Tool helpers
// ---------------------------------------------------------------------------

async fn call_tool(
    peer: &rmcp::service::Peer<rmcp::RoleClient>,
    name: &str,
    args: serde_json::Value,
) -> Result<String> {
    let params = CallToolRequestParams {
        name: std::borrow::Cow::Owned(name.to_string()),
        arguments: Some(args.as_object().cloned().unwrap_or_default()),
        meta: None,
        task: None,
    };
    let result = peer.call_tool(params).await?;
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(text)
}

async fn wait_for_message(
    peer: &rmcp::service::Peer<rmcp::RoleClient>,
) -> Result<String> {
    let args = serde_json::json!({});
    emit_tool_call("wait_for_message", &args);
    call_tool(peer, "wait_for_message", args).await
}

async fn send_message(
    peer: &rmcp::service::Peer<rmcp::RoleClient>,
    target: &str,
    content: &str,
) -> Result<String> {
    let args = serde_json::json!({"target": target, "content": content});
    emit_tool_call("send_message", &args);
    call_tool(peer, "send_message", args).await
}

// ---------------------------------------------------------------------------
// Token extraction from message content
// ---------------------------------------------------------------------------

fn extract_token(content: &str) -> Option<String> {
    // Patterns: reply with "TOKEN", reply with TOKEN, token: TOKEN, echo "TOKEN", say "TOKEN"
    let patterns = [
        r#"(?i)reply\s+with\s+"([^"]+)""#,
        r#"(?i)reply\s+with\s+(\S+)"#,
        r#"(?i)token:\s*(\S+)"#,
        r#"(?i)echo\s+"([^"]+)""#,
        r#"(?i)say\s+"([^"]+)""#,
    ];
    for pat in &patterns {
        if let Ok(re) = Regex::new(pat) {
            if let Some(caps) = re.captures(content) {
                if let Some(m) = caps.get(1) {
                    return Some(m.as_str().to_string());
                }
            }
        }
    }
    None
}

fn next_fallback_token() -> String {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("stub-reply-{seq}")
}

// ---------------------------------------------------------------------------
// Parse target from bridge message format
// ---------------------------------------------------------------------------

fn parse_target(line: &str) -> Option<String> {
    // Format: [target=#channel msg=... time=... type=...] @sender: content
    let re = Regex::new(r"\[target=(\S+)\s").ok()?;
    re.captures(line).and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

fn parse_content(line: &str) -> Option<String> {
    // After "] @sender: " comes the content. Sender may contain spaces (OS usernames);
    // do not use `\S+` here — that breaks token extraction and yields empty content.
    let re = Regex::new(r"\]\s+@([^:]+):\s*(.+)$").ok()?;
    re.captures(line).and_then(|c| c.get(2).map(|m| m.as_str().to_string()))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        emit_error(&format!("{e:#}"));
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let Args {
        mcp_config,
        prompt: _,
    } = parse_args()?;
    let (command, cmd_args) = load_mcp_config(&mcp_config)?;

    // Drain stdin in background to prevent buffer fill-up.
    // The manager writes stdin notifications but the bridge handles delivery via wait_for_message.
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let stdin = tokio::io::stdin();
        let reader = tokio::io::BufReader::new(stdin);
        let mut lines = reader.lines();
        while let Ok(Some(_line)) = lines.next_line().await {
            // consumed — bridge handles delivery
        }
    });

    // Spawn bridge as child process
    let mut child = Command::new(&command)
        .args(&cmd_args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to spawn bridge: {command}"))?;

    let child_stdout = child.stdout.take().context("No stdout from bridge child")?;
    let child_stdin = child.stdin.take().context("No stdin from bridge child")?;

    // Connect as MCP client
    let service = StubClientHandler
        .serve((child_stdout, child_stdin))
        .await
        .map_err(|e| anyhow::anyhow!("MCP handshake failed: {e}"))?;
    let peer = service.peer().clone();

    // Emit session init
    let session_id = uuid::Uuid::new_v4().to_string();
    emit_session_init(&session_id);

    let delay_ms: u64 = std::env::var("STUB_DELAY_MS")
        .unwrap_or_else(|_| "200".to_string())
        .parse()
        .unwrap_or(200);

    // Short status only — full `--prompt` can be large and may contain sensitive context.
    emit_text("Processing prompt");

    // Main loop: wait for messages, extract token or use fallback, send reply
    loop {
        let response = match wait_for_message(&peer).await {
            Ok(r) => r,
            Err(e) => {
                emit_error(&format!("wait_for_message failed: {e:#}"));
                break;
            }
        };

        if response.contains("No new messages.") {
            // No messages — loop back and wait again
            continue;
        }

        // Process each line (multiple messages may arrive). Bridge output can include
        // footers such as "Reply instructions:" — only handle real message header lines.
        for line in response.lines() {
            let line = line.trim();
            if line.is_empty() || line.contains("No new messages.") {
                continue;
            }
            if !line.starts_with("[target=") {
                continue;
            }

            let Some(target) = parse_target(line) else {
                emit_error(&format!("Could not parse target from line: {line}"));
                continue;
            };
            let content = parse_content(line).unwrap_or_default();
            let token = extract_token(&content).unwrap_or_else(next_fallback_token);

            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            emit_text(&format!("Replying with: {token}"));

            if let Err(e) = send_message(&peer, &target, &token).await {
                emit_error(&format!("send_message failed: {e:#}"));
                break;
            }
        }

        emit_turn_end();
    }

    // Clean up
    drop(peer);
    drop(service);
    let _ = child.kill().await;
    Ok(())
}
