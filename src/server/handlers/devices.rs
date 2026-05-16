//! Settings → Devices: mint / list / kick / forget / rotate for the
//! current user's user-scoped bridge token.
//!
//! All four endpoints require an authenticated `Actor` (the
//! `require_auth` middleware injects it). They operate exclusively on
//! the *current user's* bridge token; cross-user effects are not
//! possible.
//!
//! The bearer is hash-only at rest. `POST /api/devices/mint` and
//! `POST /api/devices/rotate` return the raw bearer literal in the
//! script body **exactly once**, then it's gone forever. Subsequent
//! mint calls return 410 Gone until Rotate.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::server::auth::Actor;
use crate::server::handlers::AppState;
use crate::store::auth::BridgeMachine;

/// Single device row exposed to the UI. The bearer hash is never
/// surfaced; the `id` returned is the `machine_id` (which is what the
/// kick/forget endpoints take as their path segment).
#[derive(Debug, Serialize)]
pub struct DeviceDto {
    pub machine_id: String,
    pub hostname_hint: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub disconnected_at: Option<DateTime<Utc>>,
    pub kicked_at: Option<DateTime<Utc>>,
    pub active: bool,
}

impl From<BridgeMachine> for DeviceDto {
    fn from(m: BridgeMachine) -> Self {
        let active = m.is_active();
        DeviceDto {
            machine_id: m.machine_id,
            hostname_hint: m.hostname_hint,
            first_seen_at: m.first_seen_at,
            last_seen_at: m.last_seen_at,
            disconnected_at: m.disconnected_at,
            kicked_at: m.kicked_at,
            active,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DevicesListResponse {
    /// True iff this user has an active (non-revoked) user-scoped bridge
    /// token. The UI uses this to switch the CTA between "Onboard a
    /// device" (false: mint) and "Rotate token" (true: rotate).
    pub has_token: bool,
    pub devices: Vec<DeviceDto>,
}

/// `GET /api/devices` — list the current user's onboarded devices and
/// whether a bridge token has been minted at all. Returns an envelope
/// so the UI can tell apart "no token yet" from "token exists but no
/// machine has connected."
pub async fn handle_list_devices(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
) -> Response {
    let token = match state.store.find_active_user_bridge_token(&actor.account_id) {
        Ok(t) => t,
        Err(err) => {
            warn!(err = %err, "list devices: token lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };
    let Some(token) = token else {
        return Json(DevicesListResponse {
            has_token: false,
            devices: Vec::new(),
        })
        .into_response();
    };
    match state
        .store
        .list_bridge_machines_for_token(&token.token_hash)
    {
        Ok(rows) => Json(DevicesListResponse {
            has_token: true,
            devices: rows.into_iter().map(DeviceDto::from).collect(),
        })
        .into_response(),
        Err(err) => {
            warn!(err = %err, "list devices: bridge_machines lookup failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response()
        }
    }
}

#[derive(Debug, Serialize)]
pub struct MintResponse {
    pub script: String,
    pub host: String,
}

/// `POST /api/devices/mint` — first-ever mint of the user's bridge
/// token. Returns the onboarding script with the bearer literal
/// embedded. Once the response is rendered, the raw bearer is gone.
///
/// Status codes:
/// - `200 OK` — fresh mint, script returned.
/// - `410 Gone` — the user already has a bridge token. Force Rotate.
pub async fn handle_mint_device(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
) -> Response {
    match state.store.find_active_user_bridge_token(&actor.account_id) {
        Ok(Some(_)) => {
            return (
                StatusCode::GONE,
                "bridge token already exists; call /api/devices/rotate to mint a new one",
            )
                .into_response();
        }
        Ok(None) => {}
        Err(err) => {
            warn!(err = %err, "mint device: token lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    }
    let minted = match state
        .store
        .mint_user_bridge_token(&actor.account_id, Some("All devices"))
    {
        Ok(m) => m,
        Err(err) => {
            // Concurrent mint TOCTOU: the check above passed because no
            // token existed at lookup time, but another mint racing
            // alongside us inserted one before we got here. The partial
            // unique index on api_tokens (idx_api_tokens_user_bridge_unique)
            // catches this — surface as 410 Gone, same as the not-raced
            // case.
            if err.to_string().contains("UNIQUE constraint") {
                return (
                    StatusCode::GONE,
                    "bridge token already exists; call /api/devices/rotate to mint a new one",
                )
                    .into_response();
            }
            warn!(err = %err, "mint device: token mint failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };
    let host = host_from_headers(&headers);
    let script = render_onboarding_script(&host, &minted.raw);
    (StatusCode::OK, Json(MintResponse { script, host })).into_response()
}

/// `POST /api/devices/rotate` — revoke the user's bridge token (which
/// 401s every live device on its next request) and mint a new one.
/// Returns the same script-with-bearer payload as `mint`.
pub async fn handle_rotate_device(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
) -> Response {
    // Look up the existing token. A store error here must NOT silently
    // fall through to mint — that would orphan the old token (still
    // valid) AND create a new one. Bubble up to the caller.
    let existing = match state.store.find_active_user_bridge_token(&actor.account_id) {
        Ok(t) => t,
        Err(err) => {
            warn!(err = %err, "rotate device: token lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };
    if let Some(existing) = existing {
        // Snapshot machine_ids before revoke so we can close their live
        // WS sessions with 4005 (token_revoked) after the row is
        // invalidated. Without this, the bridge stays connected on a
        // dead bearer until its next /internal/* call 401s.
        let machines = state
            .store
            .list_bridge_machines_for_token(&existing.token_hash)
            .unwrap_or_default();
        if let Err(err) = state.store.revoke_token_by_hash(&existing.token_hash) {
            warn!(err = %err, "rotate device: revoke failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
        for m in &machines {
            let n = state.bridge_registry.signal_close_for_token(
                &existing.token_hash,
                &m.machine_id,
                4005,
            );
            if n > 0 {
                tracing::info!(
                    machine_id = %m.machine_id,
                    signaled = n,
                    "rotate: live bridge signaled with 4005 token_revoked"
                );
            }
        }
    }
    let minted = match state
        .store
        .mint_user_bridge_token(&actor.account_id, Some("All devices"))
    {
        Ok(m) => m,
        Err(err) => {
            warn!(err = %err, "rotate device: mint failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };
    let host = host_from_headers(&headers);
    let script = render_onboarding_script(&host, &minted.raw);
    (StatusCode::OK, Json(MintResponse { script, host })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct DeleteDeviceQuery {
    #[serde(default)]
    pub forget: Option<String>,
}

/// `DELETE /api/devices/{machine_id}`
///
/// Default: **Kick** — mark `disconnected_at` and `kicked_at`. Future
/// reconnects with the same machine_id are rejected (4004) until
/// Forget.
///
/// With `?forget=1`: **Forget** — hard-delete the row. A future
/// reconnect re-creates it as a fresh device.
pub async fn handle_delete_device(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(machine_id): Path<String>,
    Query(q): Query<DeleteDeviceQuery>,
) -> Response {
    let token = match state.store.find_active_user_bridge_token(&actor.account_id) {
        Ok(Some(t)) => t,
        Ok(None) => return (StatusCode::NOT_FOUND, "no bridge token for user").into_response(),
        Err(err) => {
            warn!(err = %err, "delete device: token lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };
    let forget = matches!(q.forget.as_deref(), Some("1") | Some("true"));
    let result = if forget {
        state
            .store
            .forget_bridge_machine(&token.token_hash, &machine_id)
    } else {
        state
            .store
            .kick_bridge_machine(&token.token_hash, &machine_id)
    };
    match result {
        Ok(true) => {
            // Disconnect any live WS for this machine_id with WS close
            // 4004 `kicked` so the bridge sees an actionable signal
            // immediately instead of looping until a /internal/* call
            // happens to fail auth.
            let signaled =
                state
                    .bridge_registry
                    .signal_close_for_token(&token.token_hash, &machine_id, 4004);
            tracing::info!(
                machine_id = %machine_id,
                forget,
                signaled,
                "device: deleted; live bridges signaled with 4004"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "device not found").into_response(),
        Err(err) => {
            warn!(err = %err, forget, "delete device: store error");
            (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response()
        }
    }
}

/// Resolve the host the client used to reach us, for the onboarding
/// script. Reads the `Host` header and restricts to a strict
/// host-charset allowlist (`[A-Za-z0-9.:-]+`). Anything else falls
/// back to the placeholder.
///
/// Hardening: the result is interpolated into a bash heredoc the
/// operator pastes into a terminal. A Host header containing
/// shell-metacharacters or command substitutions (`$(...)`, backticks)
/// could be evaluated when the script runs. Allowlist + the quoted
/// heredoc in `render_onboarding_script` are layered defense.
fn host_from_headers(headers: &HeaderMap) -> String {
    const PLACEHOLDER: &str = "chorus.your.host";
    let raw = match headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        Some(s) => s,
        None => return PLACEHOLDER.to_string(),
    };
    if !raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '_' | '[' | ']'))
    {
        warn!(host = %raw, "devices: Host header contains unsafe characters; using placeholder");
        return PLACEHOLDER.to_string();
    }
    raw.to_string()
}

/// Build the bash script body shown to the operator. Bearer literal is
/// embedded. The Settings UI hands this to a copy-block; the operator
/// pastes into the target device's terminal.
///
/// The credentials heredoc is single-quoted (`<<'EOF'`) so bash will
/// NOT evaluate `$VAR` / `$(...)` inside the body even if `host` or
/// `bearer` somehow contained shell metacharacters. The other strings
/// (echo lines) go through `host_from_headers`' allowlist.
fn render_onboarding_script(host: &str, bearer: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${{CHORUS_INSTALL_DIR:-$HOME/.local/bin}}"
REPO="Fullstop000/Chorus"

if ! command -v chorus >/dev/null 2>&1; then
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64)   TARGET=x86_64-unknown-linux-musl ;;
    Linux-aarch64)  TARGET=aarch64-unknown-linux-musl ;;
    Linux-arm64)    TARGET=aarch64-unknown-linux-musl ;;
    Darwin-arm64)   TARGET=aarch64-apple-darwin ;;
    Darwin-x86_64)
      echo 'Intel macOS is not in the release matrix. Install from source:' >&2
      echo "  cargo install --git https://github.com/$REPO --bin chorus" >&2
      exit 1
      ;;
    *)
      echo "Unsupported platform: $(uname -s) $(uname -m)" >&2
      echo "Browse releases: https://github.com/$REPO/releases/latest" >&2
      exit 1
      ;;
  esac

  TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)
  if [ -z "$TAG" ]; then
    echo "Failed to resolve latest release tag from GitHub API." >&2
    exit 1
  fi

  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT
  URL="https://github.com/$REPO/releases/download/$TAG/chorus-$TAG-$TARGET.tar.gz"
  echo "Installing chorus $TAG ($TARGET) → $INSTALL_DIR ..."
  curl -fsSL "$URL" | tar -xz -C "$TMP"
  # Tarball layout: chorus-$TAG-$TARGET/{{chorus,chorus-server}}; copy
  # just the device binary out of the staging subdir.
  mkdir -p "$INSTALL_DIR"
  mv "$TMP"/*/chorus "$INSTALL_DIR/chorus"
  chmod +x "$INSTALL_DIR/chorus"

  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
      echo "Note: $INSTALL_DIR is not in your PATH. Add it to your shell rc to keep using \`chorus\`." >&2
      export PATH="$INSTALL_DIR:$PATH"
      ;;
  esac
fi

DATA_DIR="${{XDG_DATA_HOME:-$HOME/.local/share}}/chorus/bridge"
mkdir -p "$DATA_DIR" && chmod 700 "$DATA_DIR"
umask 077
cat > "$DATA_DIR/bridge-credentials.toml" <<'EOF'
host  = "{host}"
token = "{bearer}"
EOF

printf 'Connecting → %s …\n' "{host}"
exec chorus bridge
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_script_with_host_and_bearer() {
        let s = render_onboarding_script("chorus.test", "chrs_bridge_xyz");
        assert!(s.contains(r#"host  = "chorus.test""#));
        assert!(s.contains(r#"token = "chrs_bridge_xyz""#));
        assert!(s.contains("exec chorus bridge"));
        assert!(s.contains("command -v chorus"));
    }

    #[test]
    fn install_step_targets_the_release_pipeline() {
        // Guards against accidentally regressing back to `cargo install`
        // for native platforms or breaking the target-triple mapping.
        // The script must:
        //   - point at the Fullstop000/Chorus releases API
        //   - select one of the three targets shipped in
        //     .github/workflows/release.yml (Intel macOS deliberately
        //     out of the matrix; that case errors with a cargo-install
        //     hint)
        //   - download the versioned tarball under the resolved tag
        let s = render_onboarding_script("chorus.test", "chrs_bridge_xyz");
        assert!(s.contains(r#"REPO="Fullstop000/Chorus""#));
        assert!(s.contains("api.github.com/repos/$REPO/releases/latest"));
        for target in [
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
            "aarch64-apple-darwin",
        ] {
            assert!(s.contains(target), "missing target mapping for {target}");
        }
        assert!(s.contains("releases/download/$TAG/chorus-$TAG-$TARGET.tar.gz"));
        // Intel macOS falls back to cargo install (out of release matrix).
        assert!(s.contains("Intel macOS is not in the release matrix"));
    }

    #[test]
    fn host_falls_back_when_header_missing() {
        let h = HeaderMap::new();
        assert_eq!(host_from_headers(&h), "chorus.your.host");
    }

    #[test]
    fn host_reads_host_header() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::HOST,
            "chorus.example:3001".parse().unwrap(),
        );
        assert_eq!(host_from_headers(&h), "chorus.example:3001");
    }
}
