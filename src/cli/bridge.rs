//! `chorus bridge` — remote runtime that connects to a platform over WebSocket.
//!
//! Two-process Chorus: one process runs `chorus serve` as the platform
//! (HTTP/WS API + DB), this process runs the agent runtime and tunnels
//! lifecycle + chat over `/api/bridge/ws`. Local agents talk to an
//! embedded MCP bridge on a loopback port; that bridge proxies
//! tool-calls back to the platform's HTTP API.
//!
//! Zero-arg happy path:
//!   `chorus bridge`
//!   → reads `$XDG_DATA_HOME/chorus/bridge/bridge-credentials.toml`
//!   → derives WS / HTTP URLs from `host`
//!   → uses `token` as the WS upgrade bearer
//!   → derives `machine_id` from a persisted line if present, else from
//!     `hostname`, persists the server's echoed-back value
//!
//! Credentials file shape (written by the Settings → Devices one-liner):
//! ```toml
//! host  = "chorus.your.host"
//! token = "chrs_bridge_..."
//! # machine_id = "laptop-zht"   # written automatically after first hello
//! ```

use std::path::PathBuf;
use std::sync::Arc;

const CREDENTIALS_FILE: &str = "bridge-credentials.toml";

/// Default data dir for the bridge: `$XDG_DATA_HOME/chorus/bridge`, with
/// `$HOME/.local/share/chorus/bridge` as the XDG fallback. Matches the
/// path the Settings → Devices onboarding script writes to.
pub fn default_bridge_data_dir() -> String {
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            format!("{home}/.local/share")
        });
    format!("{base}/chorus/bridge")
}

#[derive(Debug, serde::Deserialize)]
struct BridgeCredentials {
    host: String,
    token: String,
    #[serde(default)]
    machine_id: Option<String>,
}

fn parse_credentials(toml_text: &str) -> anyhow::Result<BridgeCredentials> {
    let creds: BridgeCredentials =
        toml::from_str(toml_text).map_err(|e| anyhow::anyhow!("credentials: invalid TOML: {e}"))?;
    if creds.host.trim().is_empty() {
        anyhow::bail!("credentials: `host` is empty");
    }
    if creds.token.trim().is_empty() {
        anyhow::bail!("credentials: `token` is empty");
    }
    // Treat an empty machine_id line as if it weren't there — the
    // bridge then re-derives one from `hostname`.
    let machine_id = creds.machine_id.filter(|s| !s.trim().is_empty());
    Ok(BridgeCredentials {
        host: creds.host,
        token: creds.token,
        machine_id,
    })
}

/// Render the URL pair from a host string. `host` may include an
/// explicit scheme (`https://chorus.host`) — that's authoritative. With
/// no scheme: TLS is the default; only loopback hostnames (`localhost`,
/// `127.0.0.1`, `[::1]`) downgrade to plaintext. A production host like
/// `chorus.example.com:8443` keeps HTTPS — port presence is NOT a
/// downgrade signal.
fn derive_urls(host: &str) -> (String, String) {
    let trimmed = host.trim().trim_end_matches('/');
    // Explicit scheme: trust it. Map https↔wss / http↔ws.
    for (http_prefix, ws_scheme) in [("https://", "wss"), ("http://", "ws")] {
        if let Some(rest) = trimmed.strip_prefix(http_prefix) {
            return (
                format!("{http_prefix}{rest}"),
                format!("{ws_scheme}://{rest}/api/bridge/ws"),
            );
        }
    }
    // No scheme: bare host (possibly with port).
    let bare = trimmed;
    // Strip port. IPv6 hosts are bracketed (`[::1]` or `[::1]:8443`);
    // split on the closing bracket. Otherwise a simple `:port` strip.
    let host_only = if let Some(rest) = bare.strip_prefix('[') {
        rest.split_once(']').map(|(h, _)| h).unwrap_or(rest)
    } else if let Some((h, _)) = bare.rsplit_once(':') {
        h
    } else {
        bare
    };
    let is_loopback = matches!(host_only, "localhost" | "127.0.0.1" | "::1");
    let (http_scheme, ws_scheme) = if is_loopback {
        ("http", "ws")
    } else {
        ("https", "wss")
    };
    (
        format!("{http_scheme}://{bare}"),
        format!("{ws_scheme}://{bare}/api/bridge/ws"),
    )
}

/// Sanitize a string for use as a `machine_id`: lowercase, only
/// `[a-z0-9-]`, truncated to 32 chars. Empty input → empty output;
/// caller falls back to the random path.
fn sanitize_machine_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(32));
    for ch in raw.chars().flat_map(|c| c.to_lowercase()) {
        if out.chars().count() >= 32 {
            break;
        }
        match ch {
            'a'..='z' | '0'..='9' | '-' => out.push(ch),
            ' ' | '_' | '.' => out.push('-'),
            _ => {}
        }
    }
    out.trim_matches('-').to_string()
}

fn random_machine_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 6];
    rand::rng().fill_bytes(&mut b);
    let mut out = String::from("mch-");
    for byte in b {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Derive `machine_id` for first-hello in the order: persisted credentials,
/// `hostname` command, random fallback.
fn resolve_machine_id(persisted: Option<&str>) -> String {
    if let Some(m) = persisted {
        if !m.is_empty() {
            return m.to_string();
        }
    }
    let host = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let sanitized = sanitize_machine_id(&host);
    if !sanitized.is_empty() {
        sanitized
    } else {
        random_machine_id()
    }
}

/// Append (or update) a `machine_id` line in the credentials file. Used
/// after the first hello when the server has echoed back the assigned
/// value (which may be suffix-disambiguated vs. what we proposed).
fn persist_machine_id(credentials_path: &PathBuf, machine_id: &str) -> anyhow::Result<()> {
    // Read existing content. Distinguish "file doesn't exist" (treat
    // as empty — the bridge is being onboarded the first time) from
    // "file exists but unreadable" (permissions, IO error) — in the
    // latter case bail rather than silently clobber the host/token
    // the user just pasted.
    let existing = match std::fs::read_to_string(credentials_path) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(anyhow::anyhow!(
                "could not read {} to persist machine_id: {err}",
                credentials_path.display()
            ));
        }
    };
    let mut lines: Vec<String> = existing.lines().map(|l| l.to_string()).collect();
    let new_line = format!("machine_id = \"{machine_id}\"");
    let mut updated = false;
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("machine_id") {
            *line = new_line.clone();
            updated = true;
            break;
        }
    }
    if !updated {
        lines.push(new_line);
    }
    let body = format!("{}\n", lines.join("\n"));
    atomic_write_0600(credentials_path, body.as_bytes())?;
    Ok(())
}

/// Write `bytes` to `path` atomically via tempfile + rename. The file
/// is created with mode 0600 (bridge credentials carry a bearer
/// secret). A crash between the temp write and the rename leaves the
/// previous file intact — `std::fs::write` alone could leave a
/// half-written file containing only the `machine_id` line and no
/// host/token.
fn atomic_write_0600(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("credentials")
    ));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub async fn run(data_dir_str: String) -> anyhow::Result<()> {
    use crate::bridge::client;

    let data_dir = PathBuf::from(&data_dir_str);
    let data_subdir = data_dir.join("data");
    let agents_dir = data_dir.join("agents");
    let credentials_path = data_dir.join(CREDENTIALS_FILE);
    std::fs::create_dir_all(&data_subdir)?;
    std::fs::create_dir_all(&agents_dir)?;

    // Load credentials. Hard fail with an actionable message — without
    // host + token we have nothing to dial.
    let toml_text = std::fs::read_to_string(&credentials_path).map_err(|err| {
        anyhow::anyhow!(
            "could not read {}: {err}\n\
             Onboard this device from Settings → Devices on the platform\n\
             and paste the displayed one-liner into this terminal.",
            credentials_path.display()
        )
    })?;
    let creds = parse_credentials(&toml_text)?;

    let machine_id = resolve_machine_id(creds.machine_id.as_deref());

    // Persist the machine_id NOW (before first hello). If the server
    // echoes back a suffix-disambiguated value in a later slice, we
    // update on top of this baseline. For v1 the bridge uses its own
    // resolved id and trusts the server's accept/reject signal.
    if creds.machine_id.as_deref() != Some(&machine_id) {
        if let Err(err) = persist_machine_id(&credentials_path, &machine_id) {
            tracing::warn!(err = %err, "failed to persist machine_id; non-fatal");
        }
    }

    let (platform_http, platform_ws) = derive_urls(&creds.host);

    let db_path = data_subdir.join("chorus-bridge.db");
    let store = Arc::new(crate::store::Store::open_for_bridge(
        db_path.to_str().unwrap(),
    )?);

    let cfg = client::BridgeClientConfig {
        platform_ws,
        platform_http,
        token: Some(creds.token),
        machine_id,
        bridge_listen: "127.0.0.1:0".to_string(),
        agents_dir,
        store,
    };

    // Out-of-process bridge: a terminal auth error (401 on upgrade,
    // 4004 kicked, 4005 token_revoked) is a user-actionable signal,
    // not a transient failure. Print the message and exit 2 so process
    // supervisors stop restarting and a human notices. Any other error
    // bubbles up to clap, which exits 1.
    match client::run_bridge_client(cfg).await {
        Ok(()) => Ok(()),
        Err(err) => {
            if let Some(terminal) =
                err.downcast_ref::<crate::bridge::client::BridgeTerminalError>()
            {
                eprintln!("\n{terminal}\n");
                std::process::exit(2);
            }
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_credentials_happy_path() {
        let c = parse_credentials(
            r#"host  = "chorus.example.com"
token = "chrs_bridge_abc"
"#,
        )
        .unwrap();
        assert_eq!(c.host, "chorus.example.com");
        assert_eq!(c.token, "chrs_bridge_abc");
        assert_eq!(c.machine_id, None);
    }

    #[test]
    fn parse_credentials_picks_up_persisted_machine_id() {
        let c = parse_credentials(
            r#"host = "chorus.host"
token = "chrs_bridge_a"
machine_id = "laptop-zht"
"#,
        )
        .unwrap();
        assert_eq!(c.machine_id.as_deref(), Some("laptop-zht"));
    }

    #[test]
    fn parse_credentials_requires_host_and_token() {
        assert!(parse_credentials("token = \"x\"").is_err());
        assert!(parse_credentials("host = \"y\"").is_err());
    }

    #[test]
    fn derive_urls_for_localhost_uses_http_ws() {
        let (http, ws) = derive_urls("localhost:3001");
        assert_eq!(http, "http://localhost:3001");
        assert_eq!(ws, "ws://localhost:3001/api/bridge/ws");
    }

    #[test]
    fn derive_urls_strips_existing_scheme() {
        let (http, ws) = derive_urls("https://chorus.example.com/");
        assert_eq!(http, "https://chorus.example.com");
        assert_eq!(ws, "wss://chorus.example.com/api/bridge/ws");
    }

    #[test]
    fn derive_urls_bare_hostname_assumes_tls() {
        let (http, ws) = derive_urls("chorus.example.com");
        assert_eq!(http, "https://chorus.example.com");
        assert_eq!(ws, "wss://chorus.example.com/api/bridge/ws");
    }

    #[test]
    fn derive_urls_production_host_with_port_stays_tls() {
        // Regression for ultrareview finding #13: an explicit port on
        // a non-loopback host must NOT downgrade to plaintext.
        let (http, ws) = derive_urls("chorus.example.com:8443");
        assert_eq!(http, "https://chorus.example.com:8443");
        assert_eq!(ws, "wss://chorus.example.com:8443/api/bridge/ws");
    }

    #[test]
    fn derive_urls_loopback_aliases_use_plaintext() {
        for host in ["localhost", "127.0.0.1", "[::1]"] {
            let (http, _) = derive_urls(host);
            assert!(
                http.starts_with("http://"),
                "loopback host {host} should default to http, got {http}"
            );
        }
    }

    #[test]
    fn derive_urls_respects_explicit_http_scheme() {
        let (http, ws) = derive_urls("http://localhost:3001");
        assert_eq!(http, "http://localhost:3001");
        assert_eq!(ws, "ws://localhost:3001/api/bridge/ws");
    }

    #[test]
    fn sanitize_machine_id_lowercases_and_filters() {
        assert_eq!(sanitize_machine_id("Macintosh.local"), "macintosh-local");
        assert_eq!(sanitize_machine_id("HOMElab_01"), "homelab-01");
        assert_eq!(sanitize_machine_id(""), "");
        assert_eq!(sanitize_machine_id("!!"), "");
        let long = sanitize_machine_id(&"a".repeat(100));
        assert_eq!(long.len(), 32);
    }
}
