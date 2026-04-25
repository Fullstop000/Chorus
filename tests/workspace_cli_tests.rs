//! Integration tests for the `chorus workspace` subcommand group.

mod harness;

use std::process::Command;
use std::sync::Arc;

use chorus::store::Store;
use harness::build_router;

async fn start_fixture() -> String {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let router = build_router(store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    url
}

async fn run_workspace(server_url: &str, args: &[&str]) -> std::process::Output {
    let server_url = server_url.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_chorus"))
            .arg("workspace")
            .args(&args)
            .arg("--server-url")
            .arg(&server_url)
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
async fn workspace_create_list_switch_current_and_rename() {
    let url = start_fixture().await;

    let out = run_workspace(&url, &["current"]).await;
    assert!(
        !out.status.success(),
        "current should fail before setup/create"
    );
    assert!(
        combined(&out).contains("no active workspace"),
        "expected missing workspace guidance, got: {}",
        combined(&out)
    );

    let out = run_workspace(&url, &["create", "Acme"]).await;
    assert!(
        out.status.success(),
        "create failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(combined(&out).contains("Acme"));
    assert!(combined(&out).contains("acme"));

    let out = run_workspace(&url, &["create", "Beta"]).await;
    assert!(
        out.status.success(),
        "create beta failed: {}",
        combined(&out)
    );

    let out = run_workspace(&url, &["current"]).await;
    assert!(
        !out.status.success(),
        "create should not select a workspace automatically: {}",
        combined(&out)
    );

    let out = run_workspace(&url, &["switch", "acme"]).await;
    assert!(out.status.success(), "switch failed: {}", combined(&out));

    let out = run_workspace(&url, &["current"]).await;
    assert!(out.status.success(), "current failed: {}", combined(&out));
    let current = combined(&out);
    assert!(current.contains("Acme"), "got: {current}");
    assert!(current.contains("acme"), "got: {current}");

    let out = run_workspace(&url, &["list"]).await;
    assert!(out.status.success(), "list failed: {}", combined(&out));
    let list = combined(&out);
    assert!(list.contains("Acme"), "got: {list}");
    assert!(list.contains("Beta"), "got: {list}");
    assert!(list.contains("* Acme"), "got: {list}");
    assert!(list.contains("channels=0 agents=0 humans=1"), "got: {list}");

    let out = run_workspace(&url, &["rename", "Acme Renamed"]).await;
    assert!(out.status.success(), "rename failed: {}", combined(&out));

    let out = run_workspace(&url, &["current"]).await;
    assert!(out.status.success(), "current failed: {}", combined(&out));
    let current = combined(&out);
    assert!(current.contains("Acme Renamed"), "got: {current}");
    assert!(
        current.contains("acme"),
        "slug should remain stable, got: {current}"
    );
}
