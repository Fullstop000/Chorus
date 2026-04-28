//! Integration tests for the `chorus channel` subcommand group.
//!
//! These exercise the real binary as a subprocess against an in-process HTTP
//! fixture (same router used by `e2e_tests.rs`). That gives us a full CLI ->
//! HTTP -> server -> store round-trip, which is what the unit tests in
//! `src/cli/channel/*.rs` deliberately don't cover.

mod harness;

use std::process::Command;
use std::sync::Arc;

use chorus::store::channels::ChannelType;
use chorus::store::messages::SenderType;
use chorus::store::Store;
use harness::build_router;

/// Boot an in-process HTTP server (mirrors `e2e_tests.rs::start_test_server`).
///
/// Returns the base URL. The store handle is dropped — these tests only need
/// the HTTP surface, and dropping the Arc keeps the test bodies short.
async fn start_fixture() -> String {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.ensure_human_with_id("testuser", "testuser").unwrap();
    // Seed a default channel so `list --all` isn't empty even before we create.
    store
        .create_channel("general", Some("General"), ChannelType::Channel, None)
        .unwrap();
    store
        .join_channel("general", "testuser", SenderType::Human)
        .unwrap();
    let router = build_router(store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // Give axum a tick to start accepting. Matches the pattern in e2e_tests.rs.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    url
}

/// Run the chorus binary with `channel <args...>` and capture output.
///
/// `RUST_LOG=chorus=info` matches the CLI's default filter so the
/// `tracing::info!` success lines end up on stdout in the captured output.
///
/// Runs inside `spawn_blocking` because `Command::output()` blocks the
/// current thread — on the default `#[tokio::test]` current-thread runtime
/// that would starve the in-process fixture's axum server and hang.
async fn run_channel(args: &[&str]) -> std::process::Output {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_chorus"))
            .arg("channel")
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

/// Combined stdout+stderr — tracing defaults to stdout in this project, but
/// the anyhow error chain lands on stderr. Asserting on the union lets each
/// test state intent (success-line appears / error-line appears) without
/// pinning which stream it came from.
fn combined(out: &std::process::Output) -> String {
    let mut s = stdout_of(out);
    s.push_str(&stderr_of(out));
    s
}

#[tokio::test]
async fn channel_lifecycle() {
    let url = start_fixture().await;

    // Create
    let out = run_channel(&[
        "create",
        "lifetest",
        "--description",
        "hi",
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
    assert!(
        combined(&out).contains("Channel #lifetest created."),
        "expected creation line, got: {}",
        combined(&out)
    );

    // List --all — the freshly-created channel should be visible even if the
    // current OS user hasn't joined it. Using --all sidesteps "what's the
    // current user" coupling so this test is environment-independent.
    let out = run_channel(&["list", "--all", "--server-url", &url]).await;
    assert!(out.status.success(), "list failed: {}", stderr_of(&out));
    let listed = combined(&out);
    assert!(
        listed.contains("#lifetest"),
        "expected #lifetest in list, got: {listed}"
    );
    assert!(
        listed.contains("hi"),
        "expected description 'hi' in list, got: {listed}"
    );

    // History
    let out = run_channel(&["history", "lifetest", "--limit", "5", "--server-url", &url]).await;
    assert!(
        out.status.success(),
        "history failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );

    // Delete
    let out = run_channel(&["del", "lifetest", "--yes", "--server-url", &url]).await;
    assert!(
        out.status.success(),
        "del failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        combined(&out).contains("deleted"),
        "expected 'deleted' in output, got: {}",
        combined(&out)
    );

    // List --all again — #lifetest should be gone.
    let out = run_channel(&["list", "--all", "--server-url", &url]).await;
    assert!(out.status.success());
    let listed = combined(&out);
    assert!(
        !listed.contains("#lifetest"),
        "expected #lifetest to be gone, got: {listed}"
    );
}

#[tokio::test]
async fn channel_create_rejects_invalid_name() {
    let url = start_fixture().await;

    for bad_name in ["", "space channel", "channel/name", "emoji🎉"] {
        let out = run_channel(&[
            "create",
            bad_name,
            "--description",
            "test",
            "--server-url",
            &url,
        ])
        .await;
        assert!(
            !out.status.success(),
            "expected non-zero exit for invalid channel name: {bad_name}"
        );
        let err = combined(&out);
        assert!(
            err.contains("can only contain lowercase letters, numbers, hyphens, and underscores"),
            "expected validation error for '{bad_name}', got: {err}"
        );
    }
}

#[tokio::test]
async fn channel_delete_alias_works() {
    let url = start_fixture().await;

    // Create a channel
    let out = run_channel(&[
        "create",
        "aliastest",
        "--description",
        "for alias test",
        "--server-url",
        &url,
    ])
    .await;
    assert!(out.status.success(), "create failed: {}", stderr_of(&out));

    // Use `delete` alias (not `del`) to remove it
    let out = run_channel(&["delete", "aliastest", "--yes", "--server-url", &url]).await;
    assert!(
        out.status.success(),
        "delete alias failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        combined(&out).contains("deleted"),
        "expected 'deleted' in output, got: {}",
        combined(&out)
    );
}

#[tokio::test]
async fn channel_del_not_found() {
    let url = start_fixture().await;

    let out = run_channel(&["del", "nope", "--yes", "--server-url", &url]).await;
    assert!(
        !out.status.success(),
        "expected non-zero exit for unknown channel"
    );
    let err = combined(&out);
    assert!(
        err.contains("channel not found: #nope"),
        "expected 'channel not found: #nope' in output, got: {err}"
    );
}

#[tokio::test]
async fn channel_del_non_tty_refuses_without_yes() {
    let url = start_fixture().await;

    // Run `del` without `--yes`. Command::output inherits a non-TTY stdin,
    // so the binary should exit 1 with the refusal message rather than
    // blocking on a prompt.
    let out = run_channel(&["del", "foo", "--server-url", &url]).await;
    assert!(
        !out.status.success(),
        "expected non-zero exit when refusing non-TTY del"
    );
    let err = combined(&out);
    assert!(
        err.contains("refusing to delete #foo without --yes on non-interactive stdin"),
        "expected non-TTY refusal message, got: {err}"
    );
}

#[tokio::test]
async fn channel_list_server_unreachable() {
    // Port 1 on loopback: reliably refused by the kernel (nothing binds there,
    // and connecting as a non-root user to a privileged port is permitted —
    // only *binding* requires privilege). We strip HTTP(S)_PROXY below so a
    // corporate proxy can't turn the refusal into a 5xx response.
    let unreachable = "http://127.0.0.1:1".to_string();

    // Strip HTTP(S)_PROXY env vars so we get a genuine connection refusal
    // from the kernel rather than a proxy turning it into a 5xx.
    let args: Vec<String> = [
        "list".to_string(),
        "--server-url".to_string(),
        unreachable.clone(),
    ]
    .to_vec();
    let out = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_chorus"))
            .arg("channel")
            .args(&args)
            .env("RUST_LOG", "chorus=info")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("ALL_PROXY")
            .env_remove("all_proxy")
            .output()
            .expect("failed to run chorus binary")
    })
    .await
    .expect("spawn_blocking panicked");
    assert!(
        !out.status.success(),
        "expected non-zero exit when server is unreachable"
    );
    let err = combined(&out);
    assert!(
        err.contains(&format!("is the Chorus server running at {unreachable}")),
        "expected connection-refused context message, got: {err}"
    );
}
