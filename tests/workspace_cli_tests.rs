//! Integration tests for the `chorus workspace` subcommand group.

use std::process::Command;

fn run_workspace(data_dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_chorus"))
        .arg("workspace")
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .env("RUST_LOG", "chorus=info")
        .output()
        .expect("failed to run chorus binary")
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

#[test]
fn workspace_create_list_switch_current_and_rename() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("chorus-home");

    let out = run_workspace(&data_dir, &["current"]);
    assert!(
        !out.status.success(),
        "current should fail before setup/create"
    );
    assert!(
        combined(&out).contains("no active workspace"),
        "expected missing workspace guidance, got: {}",
        combined(&out)
    );

    let out = run_workspace(&data_dir, &["create", "Acme"]);
    assert!(
        out.status.success(),
        "create failed: stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(combined(&out).contains("Acme"));
    assert!(combined(&out).contains("acme"));

    let out = run_workspace(&data_dir, &["create", "Beta"]);
    assert!(
        out.status.success(),
        "create beta failed: {}",
        combined(&out)
    );

    let out = run_workspace(&data_dir, &["current"]);
    assert!(out.status.success(), "current failed: {}", combined(&out));
    let current = combined(&out);
    assert!(current.contains("Beta"), "got: {current}");
    assert!(current.contains("beta"), "got: {current}");

    let out = run_workspace(&data_dir, &["list"]);
    assert!(out.status.success(), "list failed: {}", combined(&out));
    let list = combined(&out);
    assert!(list.contains("Acme"), "got: {list}");
    assert!(list.contains("Beta"), "got: {list}");
    assert!(list.contains("* Beta"), "got: {list}");

    let out = run_workspace(&data_dir, &["switch", "acme"]);
    assert!(out.status.success(), "switch failed: {}", combined(&out));

    let out = run_workspace(&data_dir, &["rename", "Acme Renamed"]);
    assert!(out.status.success(), "rename failed: {}", combined(&out));

    let out = run_workspace(&data_dir, &["current"]);
    assert!(out.status.success(), "current failed: {}", combined(&out));
    let current = combined(&out);
    assert!(current.contains("Acme Renamed"), "got: {current}");
    assert!(
        current.contains("acme"),
        "slug should remain stable, got: {current}"
    );
}
