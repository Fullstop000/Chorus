//! `chorus workspace` — manage platform workspaces for the local instance.

use std::path::Path;

use clap::Subcommand;

use chorus::config::ChorusConfig;
use chorus::store::{Store, Workspace};

use super::{default_data_dir, DATA_SUBDIR};

#[derive(Subcommand)]
pub(crate) enum WorkspaceCommands {
    /// Print the active workspace
    Current,
    /// List workspaces for the local human
    List,
    /// Create a workspace and switch to it
    Create {
        /// Display name for the new workspace
        name: String,
    },
    /// Switch the active workspace by slug or exact name
    Switch {
        /// Workspace slug or exact name
        workspace: String,
    },
    /// Rename the active workspace. The slug remains stable.
    Rename {
        /// New display name
        name: String,
    },
}

pub async fn run(data_dir: Option<String>, cmd: WorkspaceCommands) -> anyhow::Result<()> {
    let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
    let data_dir = chorus::agent::templates::expand_tilde(&data_dir_str);
    let store = open_workspace_store(&data_dir)?;

    match cmd {
        WorkspaceCommands::Current => {
            let workspace = active_workspace_or_user_error(&store)?;
            print_workspace(&workspace, true);
        }
        WorkspaceCommands::List => {
            let human = resolve_local_human(&data_dir)?;
            let active = store.get_active_workspace()?;
            let workspaces = store.list_workspaces_for_human(&human)?;
            for workspace in workspaces {
                let is_active = active
                    .as_ref()
                    .is_some_and(|active| active.id == workspace.id);
                print_workspace(&workspace, is_active);
            }
        }
        WorkspaceCommands::Create { name } => {
            let human = ensure_local_human(&data_dir)?;
            let workspace = store.create_local_workspace(&name, &human)?;
            println!(
                "Created and switched to workspace {} ({})",
                workspace.name, workspace.slug
            );
        }
        WorkspaceCommands::Switch { workspace } => {
            let workspace = store
                .get_workspace_by_selector(&workspace)?
                .ok_or_else(|| workspace_not_found(&workspace))?;
            store.set_active_workspace(&workspace.id)?;
            println!(
                "Switched to workspace {} ({})",
                workspace.name, workspace.slug
            );
            println!("Restart `chorus start` to apply this workspace to a running server.");
        }
        WorkspaceCommands::Rename { name } => {
            let active = active_workspace_or_user_error(&store)?;
            let workspace = store.rename_workspace(&active.id, &name)?;
            println!(
                "Renamed workspace to {} ({})",
                workspace.name, workspace.slug
            );
            println!("Restart `chorus start` to apply this workspace to a running server.");
        }
    }

    Ok(())
}

fn open_workspace_store(data_dir: &Path) -> anyhow::Result<Store> {
    let data_subdir = data_dir.join(DATA_SUBDIR);
    std::fs::create_dir_all(&data_subdir)?;
    let db_path = data_subdir.join("chorus.db");
    Store::open(path_to_str(&db_path)?)
}

fn path_to_str(path: &Path) -> anyhow::Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", path.display()))
}

fn active_workspace_or_user_error(store: &Store) -> anyhow::Result<Workspace> {
    store.get_active_workspace()?.ok_or_else(|| {
        crate::cli::UserError(
            "no active workspace; run `chorus setup` or `chorus workspace create <name>`".into(),
        )
        .into()
    })
}

fn workspace_not_found(selector: &str) -> crate::cli::UserError {
    crate::cli::UserError(format!("workspace not found: {selector}"))
}

fn print_workspace(workspace: &Workspace, active: bool) {
    let marker = if active { "*" } else { " " };
    println!(
        "{marker} {} ({}) [{}] id={}",
        workspace.name,
        workspace.slug,
        workspace.mode.as_db_str(),
        workspace.id
    );
}

fn resolve_local_human(data_dir: &Path) -> anyhow::Result<String> {
    let cfg = ChorusConfig::load(data_dir)?.unwrap_or_default();
    Ok(cfg
        .local_human
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(whoami::username))
}

fn ensure_local_human(data_dir: &Path) -> anyhow::Result<String> {
    let mut cfg = ChorusConfig::load(data_dir)?.unwrap_or_default();
    let human = cfg
        .local_human
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(whoami::username);
    cfg.local_human.name = Some(human.clone());
    cfg.save(data_dir)?;
    Ok(human)
}
