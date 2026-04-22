//! `chorus check` — read-only environment diagnostic.
//!
//! Reports runtime installation/auth status, data-directory health, and shared
//! MCP bridge reachability without mutating the environment.

use chorus::agent::drivers::ProbeAuth;
use chorus::agent::manager::build_driver_registry;
use chorus::bridge::discovery::{read_bridge_status, BridgeStatus};
use chorus::config::ChorusConfig;
use console::{style, Emoji};
use std::path::Path;

use std::time::Duration;
use tokio::process::Command;

// Glyphs. `console::Emoji` falls back to ASCII on dumb terminals.
static OK: Emoji<'_, '_> = Emoji("✓ ", "ok ");
static BAD: Emoji<'_, '_> = Emoji("✗ ", "x  ");
static WARN: Emoji<'_, '_> = Emoji("⚠ ", "!  ");
static MISSING: Emoji<'_, '_> = Emoji("○ ", "-  ");

pub async fn run(data_dir: Option<String>) -> anyhow::Result<()> {
    let data_dir = data_dir.unwrap_or_else(super::default_data_dir);

    println!();
    println!("  {}", style("Chorus · environment check").bold());

    check_runtimes().await;
    check_data_dir(&data_dir).await;
    check_bridge().await;

    println!();
    Ok(())
}

fn section(title: &str) {
    println!();
    println!("  {}", style(title).bold());
}

fn row_ok(name: &str, detail: &str) {
    println!(
        "  {}{:<12} {}",
        style(OK).green(),
        style(name).bold(),
        style(detail).dim()
    );
}

fn row_warn(name: &str, detail: &str) {
    println!(
        "  {}{:<12} {}",
        style(WARN).yellow(),
        style(name).bold(),
        style(detail).dim()
    );
}

fn row_bad(name: &str, detail: &str) {
    println!(
        "  {}{:<12} {}",
        style(BAD).red(),
        style(name).bold(),
        style(detail).dim()
    );
}

fn row_missing(name: &str, detail: &str) {
    println!(
        "  {}{:<12} {}",
        style(MISSING).dim(),
        style(name).bold(),
        style(detail).dim()
    );
}

// ---------------------------------------------------------------------------
// Runtimes
// ---------------------------------------------------------------------------

async fn check_runtimes() {
    section("Runtimes");

    let registry = build_driver_registry();
    let mut runtimes: Vec<_> = registry.into_iter().collect();
    runtimes.sort_by_key(|(k, _)| *k);

    for (runtime, driver) in runtimes {
        let name = runtime.as_str();

        // Auth probe with timeout.
        // NOTE: several driver probes do blocking std::process::Command work
        // inside their async fn without yielding. The timeout prevents the
        // checker from hanging indefinitely, but it does not kill the
        // underlying child process — a known v1 limitation.
        let auth = match tokio::time::timeout(Duration::from_secs(5), driver.probe()).await {
            Ok(Ok(probe)) => probe.auth,
            Ok(Err(e)) => {
                row_warn(name, &format!("probe error: {e}"));
                continue;
            }
            Err(_) => {
                row_warn(name, "probe timed out");
                continue;
            }
        };

        if auth == ProbeAuth::NotInstalled {
            row_missing(name, "not found");
            continue;
        }

        let version = probe_version(name).await;
        let version_str = version.as_deref().unwrap_or("version unknown");

        match auth {
            ProbeAuth::Authed => row_ok(name, &format!("{version_str} · authenticated")),
            ProbeAuth::Unauthed => row_warn(name, &format!("{version_str} · not authenticated")),
            ProbeAuth::NotInstalled => unreachable!(),
        }
    }
}

/// Run `<binary> --version` with a 5-second timeout and extract the version string.
/// Uses `Command::output()` internally so stdout/stderr are drained concurrently,
/// avoiding the pipe-deadlock risk of `spawn()` + `wait()`.
async fn probe_version(binary: &str) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new(binary).arg("--version").output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let source = if stdout.trim().is_empty() {
        String::from_utf8_lossy(&output.stderr)
    } else {
        stdout
    };

    extract_version(&source)
}

/// Extract the first dotted version number from a tool's `--version` output.
fn extract_version(s: &str) -> Option<String> {
    static VERSION_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = VERSION_RE
        .get_or_init(|| regex::Regex::new(r"\b\d+\.\d+(?:\.\d+)?(?:[-+][\w.]+)?\b").unwrap());
    re.find(s).map(|m| m.as_str().to_string())
}

// ---------------------------------------------------------------------------
// Data directory
// ---------------------------------------------------------------------------

async fn check_data_dir(data_dir: &str) {
    section("Data");

    let data_dir_path = Path::new(data_dir);

    // Root data directory.
    match std::fs::read_dir(data_dir_path) {
        Ok(_) => row_ok("data dir", &data_dir_path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            row_missing("data dir", "not found · run \"chorus setup\" to initialize");
        }
        Err(e) => row_bad("data dir", &e.to_string()),
    }

    // Config file.
    let config_path = data_dir_path.join("config.toml");
    match ChorusConfig::load(data_dir_path) {
        Ok(Some(cfg)) => {
            row_ok("config", &config_path.display().to_string());
            if let Some(id) = &cfg.machine_id {
                row_ok("machine id", id);
            } else {
                row_missing("machine id", "not set · run \"chorus setup\" to initialize");
            }
        }
        Ok(None) => {
            row_missing("config", "not found · run \"chorus setup\" to configure");
            row_missing("machine id", "not set · run \"chorus setup\" to initialize");
        }
        Err(e) => row_bad("config", &e.to_string()),
    }

    // Data subdirectory.
    let data_sub = data_dir_path.join(super::DATA_SUBDIR);
    check_dir_readable("data", &data_sub);

    // Database file.
    let db_path = data_sub.join("chorus.db");
    match std::fs::metadata(&db_path) {
        Ok(m) if m.is_file() => row_ok("database", &db_path.display().to_string()),
        Ok(_) => row_bad("database", &format!("{} is not a file", db_path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            row_missing("database", "not found · run \"chorus setup\" to initialize");
        }
        Err(e) => row_bad("database", &e.to_string()),
    }

    // Logs directory.
    let logs_dir = data_dir_path.join("logs");
    check_dir_readable("logs", &logs_dir);

    // Agents directory.
    let agents_dir = data_dir_path.join("agents");
    check_dir_readable("agents", &agents_dir);
}

fn check_dir_readable(label: &str, path: &Path) {
    match std::fs::read_dir(path) {
        Ok(_) => row_ok(label, &path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            row_missing(label, "not found · run \"chorus setup\" to initialize");
        }
        Err(e) => row_bad(label, &e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Bridge
// ---------------------------------------------------------------------------

async fn check_bridge() {
    section("Bridge");

    match read_bridge_status() {
        BridgeStatus::Missing => {
            row_missing("bridge", "not running");
        }
        BridgeStatus::Unreadable { reason } => {
            row_bad("bridge", &format!("discovery file unreadable · {reason}"));
        }
        BridgeStatus::Invalid { reason } => {
            row_bad("bridge", &format!("discovery file invalid · {reason}"));
        }
        BridgeStatus::Stale { info } => {
            row_missing(
                "bridge",
                &format!("not running (stale discovery file · pid {})", info.pid),
            );
        }
        BridgeStatus::Live { info } => {
            let url = format!("http://127.0.0.1:{}/health", info.port);
            let start = std::time::Instant::now();
            match tokio::time::timeout(Duration::from_secs(3), reqwest::get(&url)).await {
                Ok(Ok(resp)) if resp.status().is_success() => {
                    let elapsed = start.elapsed().as_millis();
                    row_ok(
                        "bridge",
                        &format!("reachable · 127.0.0.1:{} ({}ms)", info.port, elapsed),
                    );
                }
                Ok(Ok(resp)) => {
                    row_warn("bridge", &format!("unreachable · HTTP {}", resp.status()));
                }
                Ok(Err(e)) => {
                    let msg = if e.is_connect() {
                        "connection refused"
                    } else {
                        "request failed"
                    };
                    row_warn("bridge", &format!("unreachable · {msg}"));
                }
                Err(_) => {
                    row_warn("bridge", "unreachable · timed out");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_version_finds_dotted_number() {
        assert_eq!(
            extract_version("claude, version 2.1.116"),
            Some("2.1.116".to_string())
        );
    }

    #[test]
    fn extract_version_finds_first_line_only() {
        assert_eq!(
            extract_version("codex 0.122.0\nextra line"),
            Some("0.122.0".to_string())
        );
    }

    #[test]
    fn extract_version_returns_none_when_no_number() {
        assert_eq!(extract_version("unknown tool"), None);
    }

    #[test]
    fn extract_version_handles_pre_release() {
        assert_eq!(
            extract_version("tool 1.0.0-beta.2"),
            Some("1.0.0-beta.2".to_string())
        );
    }

    #[test]
    fn check_dir_readable_existing_dir() {
        let tmp = std::env::temp_dir().join(format!("chorus_check_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        // Just verify it doesn't panic; stdout capture is awkward in unit tests.
        check_dir_readable("test", &tmp);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn check_dir_readable_missing_dir() {
        let tmp =
            std::env::temp_dir().join(format!("chorus_check_test_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir(&tmp);
        check_dir_readable("test", &tmp);
    }
}
