use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bridge discovery info written on startup, read by drivers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeInfo {
    pub port: u16,
    pub pid: u32,
    pub started_at: String, // ISO 8601
}

/// Default discovery file path: ~/.chorus/bridge.json
pub fn default_discovery_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".chorus")
        .join("bridge.json")
}

/// Write bridge info to the default discovery file. Creates ~/.chorus/ if needed.
pub fn write_bridge_info(info: &BridgeInfo) -> std::io::Result<()> {
    write_bridge_info_to(&default_discovery_path(), info)
}

/// Write bridge info to a specific path. Creates parent directories if needed.
///
/// The write is atomic: we write to a sibling `.tmp` file and rename on top of
/// the target, so a driver reading mid-write never observes a truncated file.
/// On Unix the parent directory is tightened to 0700 and the file to 0600 to
/// prevent other local users from reading the bridge port.
pub fn write_bridge_info_to(path: &std::path::Path, info: &BridgeInfo) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Best-effort: if the dir pre-exists with laxer perms we tighten
            // them; failures are non-fatal since the file mode below is the
            // primary gate.
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let json = serde_json::to_string_pretty(info)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let tmp_path = path.with_extension("json.tmp");
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        // Create the temp file with 0600 from the start so it is never
        // world-readable, even momentarily.
        let mut tmp_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp_path)?;
        tmp_file.write_all(json.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp_path, json)?;
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Read bridge info from the default discovery file. Returns None if:
/// - File doesn't exist
/// - File can't be parsed
/// - PID in file is not alive (stale bridge)
pub fn read_bridge_info() -> Option<BridgeInfo> {
    read_bridge_info_from(&default_discovery_path())
}

/// Read bridge info from a specific path. Returns None if:
/// - File doesn't exist
/// - File can't be parsed
/// - PID in file is not alive (stale bridge)
pub fn read_bridge_info_from(path: &std::path::Path) -> Option<BridgeInfo> {
    let contents = std::fs::read_to_string(path).ok()?;
    let info: BridgeInfo = serde_json::from_str(&contents).ok()?;
    if !is_pid_alive(info.pid) {
        return None;
    }
    Some(info)
}

/// Remove the discovery file (called on graceful shutdown).
pub fn remove_bridge_info() {
    let _ = std::fs::remove_file(default_discovery_path());
}

/// Remove a specific discovery file.
pub fn remove_bridge_info_from(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Check if a PID is alive by sending signal 0.
///
/// On Unix, `kill(pid, 0)` does not deliver any signal but performs the
/// same permission / existence checks as a real signal:
///   - Returns Ok  → process exists (we may or may not have permission)
///   - Returns ESRCH → no such process
///   - Returns EPERM → no permission, but process *does* exist
#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    match kill(Pid::from_raw(pid as i32), None) {
        Ok(_) => true,
        Err(Errno::EPERM) => true, // process exists, we just lack permission
        Err(_) => false,           // ESRCH or anything else → treat as dead
    }
}

/// On non-Unix platforms we cannot check process existence — assume alive to
/// avoid silently dropping valid bridge info.
#[cfg(not(unix))]
fn is_pid_alive(_pid: u32) -> bool {
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_info() -> BridgeInfo {
        BridgeInfo {
            port: 9000,
            pid: std::process::id(),
            started_at: "2026-04-16T00:00:00Z".to_string(),
        }
    }

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("chorus_discovery_test_{}", name))
    }

    #[test]
    fn write_and_read_roundtrip() {
        let path = tmp_path("roundtrip.json");
        let info = sample_info();

        write_bridge_info_to(&path, &info).expect("write should succeed");
        let read_back = read_bridge_info_from(&path).expect("read should return Some");

        assert_eq!(read_back.port, info.port);
        assert_eq!(read_back.pid, info.pid);
        assert_eq!(read_back.started_at, info.started_at);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_missing_file_returns_none() {
        let path = tmp_path("definitely_does_not_exist_xyz.json");
        // Make sure it really isn't there.
        let _ = std::fs::remove_file(&path);
        assert!(read_bridge_info_from(&path).is_none());
    }

    #[test]
    fn read_stale_pid_returns_none() {
        let path = tmp_path("stale_pid.json");
        let info = BridgeInfo {
            port: 9001,
            pid: 999_999_999,
            started_at: "2026-04-16T00:00:00Z".to_string(),
        };

        write_bridge_info_to(&path, &info).expect("write should succeed");
        assert!(
            read_bridge_info_from(&path).is_none(),
            "should return None for a non-existent PID"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_corrupt_file_returns_none() {
        let path = tmp_path("corrupt.json");
        std::fs::write(&path, b"this is not valid json {{{").expect("write should succeed");
        assert!(read_bridge_info_from(&path).is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn creates_parent_directory() {
        let base = std::env::temp_dir().join(format!(
            "chorus_discovery_test_newdir_{}",
            std::process::id()
        ));
        // Ensure it doesn't exist yet.
        let _ = std::fs::remove_dir_all(&base);

        let path = base.join("subdir").join("bridge.json");
        let info = sample_info();

        write_bridge_info_to(&path, &info).expect("write should create parent dirs and succeed");
        assert!(path.exists(), "file should have been created");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn remove_cleans_up_file() {
        // Write to temp path, verify file exists, remove, verify gone.
        let path = tmp_path("remove_test.json");
        let info = sample_info();

        write_bridge_info_to(&path, &info).expect("write should succeed");
        assert!(path.exists(), "file should exist after write");

        remove_bridge_info_from(&path);
        assert!(!path.exists(), "file should be removed after cleanup");
    }
}
