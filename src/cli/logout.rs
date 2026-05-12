//! `chorus logout`: revoke the current CLI token and delete the local
//! credentials file. Idempotent — running it twice is a no-op.

use std::path::Path;

use anyhow::{Context, Result};

use super::{credentials, default_data_dir};
use chorus::store::Store;

const DATA_SUBDIR: &str = "data";

pub async fn run(data_dir: Option<String>) -> Result<()> {
    let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
    let data_dir = Path::new(&data_dir_str);

    let Some(creds) = credentials::load(data_dir)? else {
        println!("Already logged out (no credentials file).");
        return Ok(());
    };

    let db_path = data_dir.join(DATA_SUBDIR).join("chorus.db");
    if db_path.exists() {
        let db_path_str = db_path
            .to_str()
            .with_context(|| format!("database path is not valid UTF-8: {}", db_path.display()))?;
        let store = Store::open(db_path_str)?;
        match store.revoke_token_by_raw(&creds.token) {
            Ok(true) => {
                tracing::info!("revoked CLI token in store");
            }
            Ok(false) => {
                // Token was already revoked OR never existed in this DB.
                // Either way, deleting the file is the right next step.
                tracing::info!(
                    "no active token row matched the credentials file; deleting file anyway"
                );
            }
            Err(err) => {
                // Storage failure shouldn't block local logout — we still
                // delete the file so the user isn't stuck. Surface the error
                // so they see what happened.
                tracing::warn!(err = %err, "failed to revoke token in store; deleting credentials file anyway");
            }
        }
    }

    credentials::delete(data_dir)?;
    println!("Logged out. Credentials file removed.");
    Ok(())
}
