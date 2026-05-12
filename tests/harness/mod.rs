//! Shared test helpers for integration tests.
#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;
use axum::Router;
use chorus::agent::activity_log::ActivityLogResponse;
use chorus::agent::runtime_status::{SharedRuntimeStatusProvider, SystemRuntimeStatusProvider};
use chorus::agent::AgentLifecycle;
use chorus::server::event_bus::EventBus;
use chorus::server::build_router_with_services;
use chorus::store::auth::api_tokens::hash_token;
use chorus::store::Store;
use rusqlite::params;

/// Deterministic bearer token used by every test that goes through
/// `build_router`. The matching api_tokens row is created on the fly when
/// the router is built, so every test has a real (not faked) credential
/// without needing to thread one through.
pub const TEST_AUTH_TOKEN: &str = "chrs_test_DEFAULT_TOKEN_8M9X2A3V_kPjQ";

/// Default test identity id+name. Hard-coded as "alice" because the
/// existing test suite already expects requests to come from a user
/// named alice; aligning the auto-bootstrapped identity to the same
/// name keeps assertions about message authorship, team membership,
/// and similar from breaking.
pub const TEST_USER_ID: &str = "alice";
pub const TEST_USER_NAME: &str = "alice";

/// Idempotent: ensure (users.id=alice + accounts.local + humans.id=alice)
/// plus an `api_tokens` row whose hash matches `TEST_AUTH_TOKEN`. Tests
/// that build the router via this harness can rely on the default auth
/// header being valid and the actor being "alice".
pub fn ensure_default_test_identity(store: &Store) {
    // Use the test-only `ensure_user_with_id` so the User and the legacy
    // humans mirror share a known id ("alice") rather than a random uuid.
    let _ = store.ensure_user_with_id(TEST_USER_ID, TEST_USER_NAME);
    // Insert (or ignore) the local account for this user. The partial-
    // unique index allows exactly one local account per install; if a
    // prior test setup created one for a different user, this is a
    // no-op and we fall through.
    let conn = store.conn_for_test();
    conn.execute(
        "INSERT OR IGNORE INTO accounts (id, user_id, auth_provider)
         VALUES (?1, ?2, 'local')",
        params![format!("acc_{}", TEST_USER_ID), TEST_USER_ID],
    )
    .ok();
    // Legacy humans mirror — keep id=name=alice so callers that hit the
    // `humans` table (which is still load-bearing until the redesign
    // finishes) see one row.
    conn.execute(
        "INSERT INTO humans (id, name, auth_provider) VALUES (?1, ?2, 'local')
         ON CONFLICT(id) DO UPDATE SET name = excluded.name",
        params![TEST_USER_ID, TEST_USER_NAME],
    )
    .ok();
    // The token. Bound to whatever local account exists.
    let account_id = format!("acc_{}", TEST_USER_ID);
    let token_hash = hash_token(TEST_AUTH_TOKEN);
    conn.execute(
        "INSERT OR IGNORE INTO api_tokens (token_hash, account_id, label)
         VALUES (?1, ?2, 'harness-default')",
        params![token_hash, account_id],
    )
    .ok();
}

/// Wrap an arbitrary router with the test-auth machinery: bootstrap the
/// default test identity in the store, then layer the auto-inject
/// middleware. Use this from tests that build their own router (with a
/// mock lifecycle, custom runtime statuses, custom templates, …) and
/// don't go through `build_router`.
pub fn wrap_with_test_auth(router: Router, store: &Store) -> Router {
    ensure_default_test_identity(store);
    router.layer(axum::middleware::from_fn(inject_test_auth))
}

/// Test-only middleware: if the incoming request has no `Authorization`
/// header AND no `chorus_sid` cookie, inject a `Bearer TEST_AUTH_TOKEN`
/// header. This lets the dozens of `tower::ServiceExt::oneshot` calls in
/// the existing integration tests keep working without each one threading
/// a token through — the auth layer still runs, the token is real (not a
/// mock), and the api_tokens row is real.
///
/// Tests that need to exercise the no-credential path (401 responses,
/// local-session bootstrap, …) explicitly attach their own Authorization
/// or Cookie header; this middleware leaves those requests alone.
async fn inject_test_auth(mut req: Request<Body>, next: Next) -> Response {
    let has_auth = req.headers().contains_key(header::AUTHORIZATION);
    let has_session_cookie = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("chorus_sid="))
        .unwrap_or(false);
    if !has_auth && !has_session_cookie {
        let value = format!("Bearer {TEST_AUTH_TOKEN}");
        if let Ok(hv) = HeaderValue::from_str(&value) {
            req.headers_mut().insert(header::AUTHORIZATION, hv);
        }
    }
    next.run(req).await
}

pub struct NoopLifecycle;

impl AgentLifecycle for NoopLifecycle {
    fn get_activity_log_data(
        &self,
        _agent_name: &str,
        _after_seq: Option<u64>,
    ) -> ActivityLogResponse {
        ActivityLogResponse {
            entries: vec![],
            agent_activity: "offline".to_string(),
            agent_detail: String::new(),
        }
    }

    fn process_state<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<chorus::agent::drivers::ProcessState>> + Send + 'a>>
    {
        Box::pin(async { None })
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        vec![]
    }
}

pub fn build_router(store: Arc<Store>) -> Router {
    build_router_with_lifecycle(store, Arc::new(NoopLifecycle))
}

/// Like `build_router`, but does NOT bootstrap a default test identity
/// or inject auth headers. Used by tests that intentionally exercise
/// the no-credential / no-account paths (loopback session bootstrap,
/// 401 responses, etc.).
pub fn build_router_raw(store: Arc<Store>) -> Router {
    let data_dir = unique_test_data_dir();
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).ok();
    build_router_with_services(
        store,
        Arc::new(EventBus::new()),
        data_dir,
        agents_dir,
        Arc::new(NoopLifecycle),
        Arc::new(SystemRuntimeStatusProvider::new(
            chorus::agent::manager::build_driver_registry(),
        )) as SharedRuntimeStatusProvider,
        vec![],
    )
}

pub fn build_router_with_lifecycle(
    store: Arc<Store>,
    lifecycle: Arc<dyn AgentLifecycle>,
) -> Router {
    build_router_with_lifecycle_and_dir(store, lifecycle, unique_test_data_dir())
}

pub fn build_router_with_lifecycle_and_dir(
    store: Arc<Store>,
    lifecycle: Arc<dyn AgentLifecycle>,
    data_dir: std::path::PathBuf,
) -> Router {
    ensure_default_test_identity(&store);
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).ok();
    let router = build_router_with_services(
        store,
        Arc::new(EventBus::new()),
        data_dir,
        agents_dir,
        lifecycle,
        Arc::new(SystemRuntimeStatusProvider::new(
            chorus::agent::manager::build_driver_registry(),
        )) as SharedRuntimeStatusProvider,
        vec![],
    );
    router.layer(axum::middleware::from_fn(inject_test_auth))
}

pub fn build_router_with_event_bus(store: Arc<Store>) -> (Router, Arc<EventBus>) {
    build_router_with_event_bus_and_dir(store, unique_test_data_dir())
}

pub fn build_router_with_event_bus_and_dir(
    store: Arc<Store>,
    data_dir: std::path::PathBuf,
) -> (Router, Arc<EventBus>) {
    ensure_default_test_identity(&store);
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).ok();
    let event_bus = Arc::new(EventBus::new());
    let router = build_router_with_services(
        store,
        event_bus.clone(),
        data_dir,
        agents_dir,
        Arc::new(NoopLifecycle),
        Arc::new(SystemRuntimeStatusProvider::new(
            chorus::agent::manager::build_driver_registry(),
        )) as SharedRuntimeStatusProvider,
        vec![],
    );
    (
        router.layer(axum::middleware::from_fn(inject_test_auth)),
        event_bus,
    )
}

/// Build a router for tests that exercise the bridge auth path. Mints
/// the supplied `(raw_token, machine_id)` pairs into the store's
/// `api_tokens` table so the platform-side `bridge_auth::check` finds
/// them. Returns the router. The caller passes the raw token in the
/// `Authorization: Bearer` header on its bridge-auth-enforcing
/// requests.
///
/// Does NOT layer the auto-inject auth middleware — bridge_auth tests
/// need precise control over which credential each request carries
/// (e.g. testing that a missing `Authorization` header → 401).
///
/// Mirrors the old `BridgeAuth::from_pairs` shape — bridges no longer
/// have a separate registry; everything goes through `api_tokens`.
pub fn build_router_with_bridge_tokens<I, S1, S2>(store: Arc<Store>, pairs: I) -> Router
where
    I: IntoIterator<Item = (S1, S2)>,
    S1: AsRef<str>,
    S2: AsRef<str>,
{
    ensure_default_test_identity(&store);
    // Mint each (token, machine_id) pair into api_tokens, bound to the
    // default test account.
    let account_id = format!("acc_{}", TEST_USER_ID);
    {
        let conn = store.conn_for_test();
        for (raw, machine) in pairs {
            let token_hash = hash_token(raw.as_ref());
            conn.execute(
                "INSERT OR IGNORE INTO api_tokens (token_hash, account_id, machine_id, label)
                 VALUES (?1, ?2, ?3, 'test-bridge')",
                params![token_hash, account_id, machine.as_ref()],
            )
            .ok();
        }
    }
    let data_dir = unique_test_data_dir();
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).ok();
    build_router_with_services(
        store,
        Arc::new(EventBus::new()),
        data_dir,
        agents_dir,
        Arc::new(NoopLifecycle),
        Arc::new(SystemRuntimeStatusProvider::new(
            chorus::agent::manager::build_driver_registry(),
        )) as SharedRuntimeStatusProvider,
        vec![],
    )
}

/// Per-call unique tempdir so parallel cargo tests don't collide on
/// `attachments/`, `agents/`, `teams/` under a shared path. The returned
/// path lives past the call: `keep()` consumes the `TempDir` wrapper
/// and disables its Drop cleanup, returning the underlying `PathBuf`.
/// `$TMPDIR` is reclaimed by the OS. Tests that need explicit cleanup
/// should use the `_and_dir` variants with their own `tempfile::TempDir`.
pub fn unique_test_data_dir() -> std::path::PathBuf {
    tempfile::Builder::new()
        .prefix("chorus-test-")
        .tempdir()
        .expect("create test data dir")
        .keep()
}

/// Test helper: silently insert a channel membership row without emitting
/// events or creating a system message.
pub fn join_channel_silent(store: &Store, channel_name: &str, member_id: &str, member_type: &str) {
    let conn = store.conn_for_test();
    let channel_id: String = conn
        .query_row(
            "SELECT id FROM channels WHERE name = ?1",
            params![channel_name],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq)
         VALUES (?1, ?2, ?3, 0)",
        params![channel_id, member_id, member_type],
    )
    .unwrap();
}
