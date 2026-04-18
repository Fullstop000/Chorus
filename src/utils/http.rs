//! Shared HTTP client factory for CLI-side code.
//!
//! Every CLI subcommand that talks to the Chorus server goes through [`client`]
//! so the per-request timeout lives in one place. A server that completes the
//! TCP handshake and then stalls would otherwise hang the CLI indefinitely.

use std::time::Duration;

/// Default per-request timeout for CLI HTTP calls.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Build a `reqwest::Client` with [`DEFAULT_TIMEOUT`].
///
/// The builder only fails on TLS init errors, which are not reachable with the
/// default feature set; the expect is deliberate.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .expect("reqwest client builder with static config cannot fail")
}
