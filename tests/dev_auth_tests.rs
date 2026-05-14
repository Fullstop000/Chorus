//! Integration tests for the dev-auth provider.
//!
//! These tests bypass `build_router` (which doesn't mount the dev-login
//! route unless the env says to) and instead exercise the underlying
//! pieces directly:
//!   - `load_dev_auth_config` env parsing + refuse-to-start.
//!   - The dev account find-or-create path through the store.
//!
//! End-to-end coverage of the route (POST a username, get a cookie)
//! lives in Playwright (`AUTH-002`) because it needs a full server boot
//! with the env vars set.

use chorus::server::auth::dev_login::load_dev_auth_config;
use chorus::store::Store;
use serial_test::serial;

/// Helper to set the env vars + always clear them after.
fn with_dev_auth_env<R>(flag: Option<&str>, users: Option<&str>, body: impl FnOnce() -> R) -> R {
    if let Some(v) = flag {
        std::env::set_var("CHORUS_DEV_AUTH", v);
    } else {
        std::env::remove_var("CHORUS_DEV_AUTH");
    }
    if let Some(v) = users {
        std::env::set_var("CHORUS_DEV_AUTH_USERS", v);
    } else {
        std::env::remove_var("CHORUS_DEV_AUTH_USERS");
    }
    let result = body();
    std::env::remove_var("CHORUS_DEV_AUTH");
    std::env::remove_var("CHORUS_DEV_AUTH_USERS");
    result
}

#[test]
#[serial]
fn flag_off_yields_disabled_config_even_with_allowlist_set() {
    with_dev_auth_env(None, Some("alice"), || {
        let cfg = load_dev_auth_config().unwrap();
        assert!(!cfg.enabled);
        assert!(cfg.allowed_users.is_empty());
    });
}

#[test]
#[serial]
fn flag_on_with_empty_allowlist_refuses_to_start() {
    with_dev_auth_env(Some("1"), None, || {
        let result = load_dev_auth_config();
        assert!(result.is_err(), "empty allowlist must error");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("CHORUS_DEV_AUTH_USERS"),
            "error must reference the env var: {msg}"
        );
    });
}

#[test]
#[serial]
fn flag_on_with_blank_csv_refuses_to_start() {
    with_dev_auth_env(Some("1"), Some(", , "), || {
        let result = load_dev_auth_config();
        assert!(
            result.is_err(),
            "all-blank allowlist must be treated as empty"
        );
    });
}

#[test]
#[serial]
fn flag_on_with_one_user() {
    with_dev_auth_env(Some("1"), Some("alice"), || {
        let cfg = load_dev_auth_config().unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.allowed_users, vec!["alice".to_string()]);
        assert!(cfg.permits("alice"));
        assert!(!cfg.permits("eve"));
    });
}

#[test]
#[serial]
fn flag_on_with_csv_users_trims_whitespace() {
    with_dev_auth_env(Some("1"), Some("  alice , bob , carol "), || {
        let cfg = load_dev_auth_config().unwrap();
        assert_eq!(
            cfg.allowed_users,
            vec!["alice".to_string(), "bob".to_string(), "carol".to_string()]
        );
    });
}

#[test]
fn dev_account_find_or_create_through_store() {
    let store = Store::open(":memory:").unwrap();
    // First call: create.
    let user = store.create_user("alice").unwrap();
    let acct = store
        .create_account(&user.id, "dev", Some("alice@dev.local"))
        .unwrap();
    assert_eq!(acct.auth_provider, "dev");
    assert_eq!(acct.email.as_deref(), Some("alice@dev.local"));

    // Second lookup: find.
    let found = store
        .find_account_by_provider_email("dev", "alice@dev.local")
        .unwrap()
        .unwrap();
    assert_eq!(found.id, acct.id);
}
