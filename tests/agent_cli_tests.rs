//! Integration tests for the `chorus agent` subcommand group.

mod harness;

use std::process::Command;
use std::sync::Arc;

use chorus::agent::AgentRuntime;
use chorus::store::Store;
use harness::build_router;

async fn start_fixture() -> String {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.create_human("testuser").unwrap();
    let router = build_router(store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    url
}

async fn run_agent(args: &[&str]) -> std::process::Output {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_chorus"))
            .arg("agent")
            .args(&args)
            .env("RUST_LOG", "chorus=info")
            .output()
            .expect("failed to run chorus binary")
    })
    .await
    .expect("spawn_blocking panicked")
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn combined(out: &std::process::Output) -> String {
    let mut s = stdout_of(out);
    s.push_str(&stderr_of(out));
    s
}

#[tokio::test]
async fn agent_get_not_found() {
    let url = start_fixture().await;

    let out = run_agent(&["get", "nope", "--server-url", &url]).await;
    assert!(
        !out.status.success(),
        "expected non-zero exit for unknown agent"
    );
    let err = combined(&out);
    assert!(
        err.contains("agent not found: nope"),
        "expected 'agent not found: nope' in output, got: {err}"
    );
}

#[tokio::test]
async fn agent_start_not_found() {
    let url = start_fixture().await;

    let out = run_agent(&["start", "nope", "--server-url", &url]).await;
    assert!(
        !out.status.success(),
        "expected non-zero exit for unknown agent"
    );
    let err = combined(&out);
    assert!(
        err.contains("agent not found: nope"),
        "expected 'agent not found: nope' in output, got: {err}"
    );
}

#[tokio::test]
async fn agent_restart_not_found() {
    let url = start_fixture().await;

    let out = run_agent(&["restart", "nope", "--server-url", &url]).await;
    assert!(
        !out.status.success(),
        "expected non-zero exit for unknown agent"
    );
    let err = combined(&out);
    assert!(
        err.contains("agent not found: nope"),
        "expected 'agent not found: nope' in output, got: {err}"
    );
}

#[tokio::test]
async fn agent_delete_not_found() {
    let url = start_fixture().await;

    let out = run_agent(&["delete", "nope", "--yes", "--server-url", &url]).await;
    assert!(
        !out.status.success(),
        "expected non-zero exit for unknown agent"
    );
    let err = combined(&out);
    assert!(
        err.contains("agent not found: nope"),
        "expected 'agent not found: nope' in output, got: {err}"
    );
}

#[tokio::test]
async fn agent_delete_refuses_non_interactive_without_yes() {
    let url = start_fixture().await;

    let out = run_agent(&["delete", "nope", "--server-url", &url]).await;
    assert!(
        !out.status.success(),
        "expected non-zero exit when refusing non-interactive delete"
    );
    let err = combined(&out);
    assert!(
        err.contains("refusing to delete @nope without --yes on non-interactive stdin"),
        "expected non-interactive refusal message, got: {err}"
    );
}

#[tokio::test]
async fn agent_crud_lifecycle() {
    let url = start_fixture().await;

    // Create
    let out = run_agent(&[
        "create",
        "testbot",
        "--runtime",
        AgentRuntime::Claude.as_str(),
        "--model",
        "sonnet",
        "--server-url",
        &url,
    ])
    .await;
    assert!(
        out.status.success(),
        "create failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );

    // Get
    let out = run_agent(&["get", "testbot", "--server-url", &url]).await;
    assert!(
        out.status.success(),
        "get failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let out_str = combined(&out);
    assert!(
        out_str.contains("testbot"),
        "expected 'testbot' in get output, got: {out_str}"
    );
    assert!(
        out_str.contains("sonnet"),
        "expected 'sonnet' in get output, got: {out_str}"
    );

    // Start (noop lifecycle accepts it)
    let out = run_agent(&["start", "testbot", "--server-url", &url]).await;
    assert!(
        out.status.success(),
        "start failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        combined(&out).contains("started"),
        "expected 'started' in output, got: {}",
        combined(&out)
    );

    // Restart
    let out = run_agent(&["restart", "testbot", "--server-url", &url]).await;
    assert!(
        out.status.success(),
        "restart failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        combined(&out).contains("restarted"),
        "expected 'restarted' in output, got: {}",
        combined(&out)
    );

    // Restart with kebab-case mode
    let out = run_agent(&[
        "restart",
        "testbot",
        "--mode",
        "reset-session",
        "--server-url",
        &url,
    ])
    .await;
    assert!(
        out.status.success(),
        "restart reset-session failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );

    // Restart with snake_case mode (alias)
    let out = run_agent(&[
        "restart",
        "testbot",
        "--mode",
        "reset_session",
        "--server-url",
        &url,
    ])
    .await;
    assert!(
        out.status.success(),
        "restart reset_session failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        combined(&out).contains("restarted"),
        "expected 'restarted' in output, got: {}",
        combined(&out)
    );

    // Delete without --yes should refuse on non-interactive stdin
    let out = run_agent(&["delete", "testbot", "--server-url", &url]).await;
    assert!(
        !out.status.success(),
        "expected non-zero exit for delete without --yes on non-interactive stdin"
    );
    let err = combined(&out);
    assert!(
        err.contains("refusing to delete"),
        "expected refusal message, got: {err}"
    );

    // Delete with --yes
    let out = run_agent(&["delete", "testbot", "--yes", "--server-url", &url]).await;
    assert!(
        out.status.success(),
        "delete failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        combined(&out).contains("deleted"),
        "expected 'deleted' in output, got: {}",
        combined(&out)
    );

    // Get after delete should fail
    let out = run_agent(&["get", "testbot", "--server-url", &url]).await;
    assert!(!out.status.success(), "expected non-zero exit after delete");
}
