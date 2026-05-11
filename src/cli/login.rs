//! `chorus login --local`: mint a fresh CLI bearer token against the
//! singleton local Account.
//!
//! Operates directly on the local SQLite store, not through the server.
//! Local-mode auth is a property of the on-disk DB; whoever owns the
//! files owns the identity, so going through the network is unnecessary
//! and would create a chicken-and-egg problem (you'd need credentials to
//! request credentials).

use std::path::Path;

use anyhow::{Context, Result};

use super::{credentials, default_data_dir, UserError};
use chorus::store::Store;

const DATA_SUBDIR: &str = "data";

pub async fn run(data_dir: Option<String>, label: Option<String>) -> Result<()> {
    let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
    let data_dir = Path::new(&data_dir_str);

    if credentials::load(data_dir)?.is_some() {
        return Err(UserError(format!(
            "credentials already present at {}; run `chorus logout` first if you want a fresh token",
            credentials::path_for(data_dir).display()
        ))
        .into());
    }

    let db_path = data_dir.join(DATA_SUBDIR).join("chorus.db");
    if !db_path.exists() {
        return Err(UserError(format!(
            "no Chorus database at {}; run `chorus setup` first",
            db_path.display()
        ))
        .into());
    }
    let db_path_str = db_path
        .to_str()
        .with_context(|| format!("database path is not valid UTF-8: {}", db_path.display()))?;
    let store = Store::open(db_path_str)?;

    let account = store
        .get_local_account()?
        .ok_or_else(|| UserError("no local account; run `chorus setup` first".into()))?;
    if account.disabled_at.is_some() {
        return Err(UserError("local account is disabled".into()).into());
    }

    let minted = store.mint_token(
        &account.id,
        "local",
        Some(label.as_deref().unwrap_or("Local CLI")),
    )?;
    let creds = credentials::Credentials {
        token: minted.raw,
        server: credentials::default_local_server(),
    };
    let path = credentials::save(data_dir, &creds)?;
    println!("Logged in. Credentials written to {} (mode 0600).", path.display());
    Ok(())
}
