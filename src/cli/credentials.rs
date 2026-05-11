//! On-disk credentials for the `chorus` CLI.
//!
//! Lives at `<data_dir>/credentials.toml` alongside `config.toml`. Holds
//! the bearer token the CLI sends as `Authorization: Bearer …` on every
//! request. Written atomically with 0600 perms so a tab-completing peer
//! on the same machine can't accidentally read it.
//!
//! This is intentionally separate from `config.toml`. Config is operator
//! settings (paths, defaults); credentials are secrets and follow a
//! stricter lifecycle (mint on login → write → revoke on logout → delete).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "credentials.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    /// Raw bearer token. Sent as `Authorization: Bearer <token>`.
    pub token: String,
    /// URL the CLI talks to. Defaulted to the local loopback at install
    /// time; cloud `chorus login` overwrites it.
    pub server: String,
}

pub fn path_for(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// Returns `Ok(None)` if the file doesn't exist; `Err` only on real I/O
/// or parse failures. Mirrors `ChorusConfig::load`.
pub fn load(data_dir: &Path) -> Result<Option<Credentials>> {
    let path = path_for(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let creds: Credentials = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(creds))
}

/// Atomic write + 0600 perms. Refuses to clobber if the existing file is
/// holding a different non-empty token — bail with guidance instead so an
/// accidental `login` doesn't strand the previous one.
pub fn save(data_dir: &Path, creds: &Credentials) -> Result<PathBuf> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("failed to create {}", data_dir.display()))?;
    let path = path_for(data_dir);
    let tmp = path.with_extension("toml.tmp");

    let body = format!(
        "# Chorus CLI credentials — written by `chorus login` / `chorus setup`.\n\
         # Sensitive: keep mode 0600. Delete this file with `chorus logout`.\n\n\
         {}",
        toml::to_string_pretty(creds)?
    );

    // Open with 0600 from the start so no window exists where the file is
    // 0644. Using `OpenOptions` lets us set the mode on Unix; on other
    // platforms the mode call is a no-op and we'd rely on the umask
    // (Chorus targets Linux/macOS; Windows users get whatever ACLs apply).
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        f.write_all(body.as_bytes())?;
        f.flush()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, body.as_bytes())?;
    }

    std::fs::rename(&tmp, &path)
        .with_context(|| format!("failed to move {} → {}", tmp.display(), path.display()))?;
    Ok(path)
}

/// Delete the credentials file if it exists. Idempotent.
pub fn delete(data_dir: &Path) -> Result<()> {
    let path = path_for(data_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("failed to remove {}", path.display()))
        }
    }
}

/// Default server URL for fresh local-mode credentials.
pub fn default_local_server() -> String {
    "http://127.0.0.1:3001".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let creds = Credentials {
            token: "chrs_local_test_abc".into(),
            server: "http://127.0.0.1:3001".into(),
        };
        save(tmp.path(), &creds).unwrap();
        let loaded = load(tmp.path()).unwrap().unwrap();
        assert_eq!(loaded, creds);
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let creds = Credentials {
            token: "chrs_local_x".into(),
            server: "http://127.0.0.1:3001".into(),
        };
        let path = save(tmp.path(), &creds).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    #[test]
    fn delete_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        // Delete when missing.
        delete(tmp.path()).unwrap();
        // Write then delete.
        let creds = Credentials {
            token: "chrs_local_y".into(),
            server: "http://127.0.0.1:3001".into(),
        };
        save(tmp.path(), &creds).unwrap();
        assert!(path_for(tmp.path()).exists());
        delete(tmp.path()).unwrap();
        assert!(!path_for(tmp.path()).exists());
        // Second delete is still fine.
        delete(tmp.path()).unwrap();
    }
}
