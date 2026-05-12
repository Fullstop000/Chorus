//! `chorus login --local`: mint a fresh CLI bearer token against the
//! singleton local Account.
//!
//! Operates directly on the local SQLite store, not through the server.
//! Local-mode auth is a property of the on-disk DB; whoever owns the
//! files owns the identity, so going through the network is unnecessary
//! and would create a chicken-and-egg problem (you'd need credentials to
//! request credentials).
//!
//! `setup` calls `mint_local_credentials` directly after creating the
//! identity rows, so the credentials-file write lives in exactly one
//! place — there's no shadow implementation in setup.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{credentials, default_data_dir, CliError};
use chorus::store::Store;

const DATA_SUBDIR: &str = "data";

/// Mint a CLI token bound to the singleton local Account and write it to
/// `credentials.toml`. Refuses if a credentials file already exists —
/// callers must `chorus logout` first to roll, OR call this only when
/// they know the file is absent (`setup` after a fresh identity).
///
/// Returns the credentials-file path on success.
pub fn mint_local_credentials(store: &Store, data_dir: &Path, label: &str) -> Result<PathBuf> {
    if credentials::load(data_dir)?.is_some() {
        return Err(CliError(format!(
            "credentials already present at {}; run `chorus logout` first if you want a fresh token",
            credentials::path_for(data_dir).display()
        ))
        .into());
    }
    let account = local_account(store)?;
    let minted = store.mint_token(&account.id, "local", Some(label))?;
    let creds = credentials::Credentials {
        token: minted.raw,
        server: credentials::default_local_server(),
    };
    credentials::save(data_dir, &creds)
}

/// Mint a bridge token bound to (local Account, machine_id) and write
/// it to `bridge-credentials.toml`. Refuses if the file already exists.
///
/// Used by setup (to provision the in-process bridge) and by an
/// eventual `chorus tokens mint --bridge` admin command.
pub fn mint_local_bridge_credentials(
    store: &Store,
    data_dir: &Path,
    machine_id: &str,
    label: &str,
) -> Result<PathBuf> {
    if credentials::bridge_load(data_dir)?.is_some() {
        return Err(CliError(format!(
            "bridge credentials already present at {}; delete the file to roll",
            credentials::bridge_path_for(data_dir).display()
        ))
        .into());
    }
    let account = local_account(store)?;
    let minted = store.mint_bridge_token(&account.id, machine_id, Some(label))?;
    let creds = credentials::BridgeCredentials {
        token: minted.raw,
        machine_id: machine_id.to_string(),
        server: credentials::default_local_server(),
    };
    credentials::bridge_save(data_dir, &creds)
}

fn local_account(store: &Store) -> Result<chorus::store::auth::Account> {
    let account = store
        .get_local_account()?
        .ok_or_else(|| CliError("no local account; run `chorus setup` first".into()))?;
    if account.disabled_at.is_some() {
        return Err(CliError("local account is disabled".into()).into());
    }
    Ok(account)
}

pub async fn run(data_dir: Option<String>, label: Option<String>) -> Result<()> {
    let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
    let data_dir = Path::new(&data_dir_str);

    let db_path = data_dir.join(DATA_SUBDIR).join("chorus.db");
    if !db_path.exists() {
        return Err(CliError(format!(
            "no Chorus database at {}; run `chorus setup` first",
            db_path.display()
        ))
        .into());
    }
    let db_path_str = db_path
        .to_str()
        .with_context(|| format!("database path is not valid UTF-8: {}", db_path.display()))?;
    let store = Store::open(db_path_str)?;

    let path = mint_local_credentials(&store, data_dir, label.as_deref().unwrap_or("Local CLI"))?;
    println!(
        "Logged in. Credentials written to {} (mode 0600).",
        path.display()
    );
    Ok(())
}
