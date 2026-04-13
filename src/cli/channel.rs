//! `chorus channel <name>` — create a new channel and join it as the OS user.
//!
//! Writes directly to the local SQLite store (no running server required).
//! Uses `--data-dir` (or `~/.chorus` by default) to locate the database.

use chorus::store::channels::ChannelType;
use chorus::store::messages::SenderType;
use chorus::store::Store;

use super::{db_path_for, default_data_dir};

pub fn run(
    name: String,
    description: Option<String>,
    data_dir: Option<String>,
) -> anyhow::Result<()> {
    let username = whoami::username();
    let data_dir = data_dir.unwrap_or_else(default_data_dir);
    let db_path = db_path_for(&data_dir);
    let store = Store::open(&db_path)?;
    store.create_channel(&name, description.as_deref(), ChannelType::Channel)?;
    store.join_channel(&name, &username, SenderType::Human)?;
    tracing::info!("Channel #{name} created.");
    Ok(())
}
