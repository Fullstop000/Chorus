//! Dev auth provider — finds-or-creates a session for an allowlisted
//! username. Mounted ONLY when `CHORUS_DEV_AUTH=1`. Refuses to start at
//! all if `CHORUS_DEV_AUTH_USERS` is empty/unset; see `dev_auth_config`.
//!
//! Security model: pure network-access-control. Anyone who can reach the
//! endpoint and supplies an allowlisted username gets a session. This is
//! intended for solo operators on access-controlled hosts (GCP install,
//! homelab) where standing up real OAuth isn't worth the cost. NOT
//! suitable for hosts reachable from the open internet without further
//! access control.
//!
//! Sidecar effects when enabled, all documented in
//! `docs/plan/dev-auth-and-bridge-onboarding.md` §3.2:
//!   - WARN log on `chorus serve` startup
//!   - Non-dismissible yellow banner in the UI
//!   - `/health` reports `"dev_auth": true`

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::server::auth::SESSION_COOKIE_NAME;
use crate::server::handlers::AppState;

const DEV_AUTH_ENABLED_ENV: &str = "CHORUS_DEV_AUTH";
const DEV_AUTH_USERS_ENV: &str = "CHORUS_DEV_AUTH_USERS";
/// Synthetic email column value for dev accounts. The `UNIQUE(auth_provider,
/// email)` index turns `<username>` into a stable lookup key for
/// `find_account_by_provider_email`.
fn dev_email_for(username: &str) -> String {
    format!("{username}@dev.local")
}

/// Resolved config for dev-auth. Returned by [`load_dev_auth_config`].
#[derive(Debug, Clone)]
pub struct DevAuthConfig {
    /// True if `CHORUS_DEV_AUTH=1` was set.
    pub enabled: bool,
    /// Allowlist of usernames that can log in. Empty when `enabled=false`.
    pub allowed_users: Vec<String>,
}

impl DevAuthConfig {
    pub fn permits(&self, username: &str) -> bool {
        self.enabled && self.allowed_users.iter().any(|u| u == username)
    }
}

/// Parse `CHORUS_DEV_AUTH` + `CHORUS_DEV_AUTH_USERS` from the environment.
/// Returns `Err` only when `CHORUS_DEV_AUTH=1` but the allowlist is empty
/// — that's the refuse-to-start case in `chorus serve`. When the flag is
/// not set, returns `Ok(DevAuthConfig { enabled: false, .. })`.
pub fn load_dev_auth_config() -> Result<DevAuthConfig, &'static str> {
    let enabled = matches!(
        std::env::var(DEV_AUTH_ENABLED_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    );
    if !enabled {
        return Ok(DevAuthConfig {
            enabled: false,
            allowed_users: Vec::new(),
        });
    }
    let raw = std::env::var(DEV_AUTH_USERS_ENV).unwrap_or_default();
    let allowed_users: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if allowed_users.is_empty() {
        return Err(
            "CHORUS_DEV_AUTH=1 set but CHORUS_DEV_AUTH_USERS is empty — \
             refusing to start. An empty allowlist means nobody can log in.",
        );
    }
    Ok(DevAuthConfig {
        enabled: true,
        allowed_users,
    })
}

#[derive(Debug, Deserialize)]
pub struct DevLoginRequest {
    pub username: String,
}

#[derive(Debug, Serialize)]
pub struct DevLoginResponse {
    pub user: DevLoginUser,
}

#[derive(Debug, Serialize)]
pub struct DevLoginUser {
    pub id: String,
    pub name: String,
}

/// `POST /api/auth/dev-login`. Mounted only when `CHORUS_DEV_AUTH=1`.
///
/// On success: sets `chorus_sid` cookie (HttpOnly, SameSite=Strict,
/// Path=/) and returns the user. 403 on disallowed username.
pub async fn handle_dev_login(
    State(state): State<AppState>,
    Json(req): Json<DevLoginRequest>,
) -> Response {
    let username = req.username.trim();
    if username.is_empty() {
        return (StatusCode::BAD_REQUEST, "username is required").into_response();
    }

    if !state.dev_auth.permits(username) {
        warn!(
            username = %username,
            "dev-login: rejecting username not in CHORUS_DEV_AUTH_USERS allowlist"
        );
        return (StatusCode::FORBIDDEN, "username not in allowlist").into_response();
    }

    let store = state.store.as_ref();
    let email = dev_email_for(username);

    // Find-or-create the (user, dev account) pair.
    let account = match store.find_account_by_provider_email("dev", &email) {
        Ok(Some(acct)) => acct,
        Ok(None) => {
            // Mint a fresh User + Account.
            let user = match store.create_user(username) {
                Ok(u) => u,
                Err(err) => {
                    warn!(err = %err, "dev-login: create_user failed");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
                }
            };
            match store.create_account(&user.id, "dev", Some(&email)) {
                Ok(a) => a,
                Err(err) => {
                    warn!(err = %err, "dev-login: create_account failed");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
                }
            }
        }
        Err(err) => {
            warn!(err = %err, "dev-login: account lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };

    if account.disabled_at.is_some() {
        return (StatusCode::FORBIDDEN, "account is disabled").into_response();
    }

    let user = match store.get_user_by_id(&account.user_id) {
        Ok(Some(u)) => u,
        Ok(None) => {
            warn!(
                user_id = %account.user_id,
                "dev-login: account points to non-existent user"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "user not found for account",
            )
                .into_response();
        }
        Err(err) => {
            warn!(err = %err, "dev-login: user lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };

    // Match local-session: D1=A, no expiry. Cloud OAuth flows can set one.
    let session = match store.create_session(&account.id, None) {
        Ok(s) => s,
        Err(err) => {
            warn!(err = %err, "dev-login: session insert failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };

    let cookie = format!(
        "{SESSION_COOKIE_NAME}={}; Path=/; HttpOnly; SameSite=Strict",
        session.id
    );
    let mut headers = HeaderMap::new();
    match HeaderValue::from_str(&cookie) {
        Ok(val) => {
            headers.insert(header::SET_COOKIE, val);
        }
        Err(err) => {
            warn!(err = %err, "dev-login: failed to build cookie header");
            return (StatusCode::INTERNAL_SERVER_ERROR, "cookie build failed").into_response();
        }
    }

    (
        StatusCode::OK,
        headers,
        Json(DevLoginResponse {
            user: DevLoginUser {
                id: user.id,
                name: user.name,
            },
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_only_users_on_allowlist() {
        let cfg = DevAuthConfig {
            enabled: true,
            allowed_users: vec!["alice".to_string(), "bob".to_string()],
        };
        assert!(cfg.permits("alice"));
        assert!(cfg.permits("bob"));
        assert!(!cfg.permits("eve"));
        assert!(!cfg.permits(""));
    }

    #[test]
    fn disabled_config_permits_nobody() {
        let cfg = DevAuthConfig {
            enabled: false,
            allowed_users: vec!["alice".to_string()],
        };
        assert!(!cfg.permits("alice"));
    }

    #[test]
    fn dev_email_for_is_stable() {
        assert_eq!(dev_email_for("alice"), "alice@dev.local");
        assert_eq!(dev_email_for("zht"), "zht@dev.local");
    }
}
