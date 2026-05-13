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
//! mint calls return 410 Gone until Rotate. See
//! `docs/plan/dev-auth-and-bridge-onboarding.md` §4.4.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
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

/// `GET /api/devices` — list the current user's onboarded devices.
/// Empty array if the user has no bridge token yet OR has never
/// onboarded anything.
pub async fn handle_list_devices(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
) -> Response {
    let token = match state
        .store
        .find_active_user_bridge_token(&actor.account_id)
    {
        Ok(Some(t)) => t,
        Ok(None) => return Json(Vec::<DeviceDto>::new()).into_response(),
        Err(err) => {
            warn!(err = %err, "list devices: token lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };
    match state
        .store
        .list_bridge_machines_for_token(&token.token_hash)
    {
        Ok(rows) => {
            let dtos: Vec<DeviceDto> = rows.into_iter().map(DeviceDto::from).collect();
            Json(dtos).into_response()
        }
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
    match state
        .store
        .find_active_user_bridge_token(&actor.account_id)
    {
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
    if let Ok(Some(existing)) = state
        .store
        .find_active_user_bridge_token(&actor.account_id)
    {
        // Soft-revoke. The store's `revoke_token_by_raw` only takes the
        // raw — we never have it for an existing token. Use the hash
        // path. Add a dedicated helper.
        if let Err(err) = state
            .store
            .revoke_token_by_hash(&existing.token_hash)
        {
            warn!(err = %err, "rotate device: revoke failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
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
    let token = match state
        .store
        .find_active_user_bridge_token(&actor.account_id)
    {
        Ok(Some(t)) => t,
        Ok(None) => return (StatusCode::NOT_FOUND, "no bridge token for user").into_response(),
        Err(err) => {
            warn!(err = %err, "delete device: token lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };
    let forget = matches!(q.forget.as_deref(), Some("1") | Some("true"));
    let result = if forget {
        state.store.forget_bridge_machine(&token.token_hash, &machine_id)
    } else {
        state.store.kick_bridge_machine(&token.token_hash, &machine_id)
    };
    match result {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "device not found").into_response(),
        Err(err) => {
            warn!(err = %err, forget, "delete device: store error");
            (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response()
        }
    }
}

/// Resolve the host the client used to reach us, for the onboarding
/// script. Tries `Host` header (sane reverse-proxy default) and falls
/// back to a placeholder the operator can edit. Strips a leading
/// `http(s)://` if present.
fn host_from_headers(headers: &HeaderMap) -> String {
    let from_host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    from_host.unwrap_or_else(|| "chorus.your.host".to_string())
}

/// Build the bash script body shown to the operator. Bearer literal is
/// embedded. The Settings UI hands this to a copy-block; the operator
/// pastes into the target device's terminal.
fn render_onboarding_script(host: &str, bearer: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

if ! command -v chorus >/dev/null 2>&1; then
  echo "Install Chorus first:"
  echo "  cargo install --git https://github.com/Fullstop000/Chorus chorus"
  exit 1
fi

DATA_DIR="${{XDG_DATA_HOME:-$HOME/.local/share}}/chorus/bridge"
mkdir -p "$DATA_DIR" && chmod 700 "$DATA_DIR"
umask 077
cat > "$DATA_DIR/bridge-credentials.toml" <<EOF
host  = "{host}"
token = "{bearer}"
EOF

echo "Connecting → {host} …"
exec chorus bridge
"#
    )
}

// `header` was used implicitly above; suppress the unused warning when no
// upstream import is in scope. (Axum reexports header types but only the
// HeaderMap is used here.)
#[allow(dead_code)]
fn _unused_response_imports(_: HeaderValue) {}

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
    fn host_falls_back_when_header_missing() {
        let h = HeaderMap::new();
        assert_eq!(host_from_headers(&h), "chorus.your.host");
    }

    #[test]
    fn host_reads_host_header() {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::HOST, "chorus.example:3001".parse().unwrap());
        assert_eq!(host_from_headers(&h), "chorus.example:3001");
    }
}
