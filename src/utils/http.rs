//! Shared HTTP client factory for CLI-side code.
//!
//! Every CLI subcommand that talks to the Chorus server goes through [`client`]
//! so the per-request timeout lives in one place. A server that completes the
//! TCP handshake and then stalls would otherwise hang the CLI indefinitely.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

/// Default per-request timeout for CLI HTTP calls.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Build a `reqwest::Client` with [`DEFAULT_TIMEOUT`] and no default auth
/// header. Use [`authed_client`] for CLI commands that need to talk to
/// the platform — every `/api/*` and `/internal/*` endpoint requires
/// authentication now.
///
/// The builder only fails on TLS init errors, which are not reachable with the
/// default feature set; the expect is deliberate.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .expect("reqwest client builder with static config cannot fail")
}

/// Build a `reqwest::Client` that automatically sends
/// `Authorization: Bearer <token>` on every request. Every CLI command
/// that talks to the platform should call this so individual request
/// builders don't need `.bearer_auth(&token)`.
pub fn authed_client(token: &str) -> reqwest::Client {
    let mut headers = HeaderMap::new();
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
        .expect("bearer token contains only ASCII chars");
    value.set_sensitive(true);
    headers.insert(AUTHORIZATION, value);
    reqwest::Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .default_headers(headers)
        .build()
        .expect("reqwest client builder with static config cannot fail")
}
