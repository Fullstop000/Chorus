//! `chorus workspace` — manage platform workspaces through the running server.

use anyhow::Context;
use clap::Subcommand;
use serde::Deserialize;

use chorus::utils::http;

#[derive(Subcommand)]
pub(crate) enum WorkspaceCommands {
    /// Print the active workspace
    Current,
    /// List workspaces
    List,
    /// Create a workspace
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

#[derive(Debug, Deserialize)]
struct WorkspaceDto {
    id: String,
    name: String,
    slug: String,
    mode: String,
    active: bool,
    channel_count: i64,
    agent_count: i64,
    human_count: i64,
}

pub async fn run(server_url: String, cmd: WorkspaceCommands) -> anyhow::Result<()> {
    let client = http::client();
    match cmd {
        WorkspaceCommands::Current => {
            let workspace = get_workspace(&client, &server_url, "/api/workspaces/current").await?;
            print_workspace(&workspace);
        }
        WorkspaceCommands::List => {
            let workspaces = get_workspaces(&client, &server_url, "/api/workspaces").await?;
            for workspace in workspaces {
                print_workspace(&workspace);
            }
        }
        WorkspaceCommands::Create { name } => {
            let workspace = post_workspace(
                &client,
                &server_url,
                "/api/workspaces",
                serde_json::json!({ "name": name }),
            )
            .await?;
            println!("Created workspace {} ({})", workspace.name, workspace.slug);
        }
        WorkspaceCommands::Switch { workspace } => {
            let workspace = post_workspace(
                &client,
                &server_url,
                "/api/workspaces/switch",
                serde_json::json!({ "workspace": workspace }),
            )
            .await?;
            println!(
                "Switched to workspace {} ({})",
                workspace.name, workspace.slug
            );
        }
        WorkspaceCommands::Rename { name } => {
            let workspace = patch_workspace(
                &client,
                &server_url,
                "/api/workspaces/current",
                serde_json::json!({ "name": name }),
            )
            .await?;
            println!(
                "Renamed workspace to {} ({})",
                workspace.name, workspace.slug
            );
        }
    }

    Ok(())
}

async fn get_workspace(
    client: &reqwest::Client,
    server_url: &str,
    path: &str,
) -> anyhow::Result<WorkspaceDto> {
    request(client.get(format!("{server_url}{path}")), server_url, path).await
}

async fn get_workspaces(
    client: &reqwest::Client,
    server_url: &str,
    path: &str,
) -> anyhow::Result<Vec<WorkspaceDto>> {
    request(client.get(format!("{server_url}{path}")), server_url, path).await
}

async fn post_workspace(
    client: &reqwest::Client,
    server_url: &str,
    path: &str,
    body: serde_json::Value,
) -> anyhow::Result<WorkspaceDto> {
    request(
        client.post(format!("{server_url}{path}")).json(&body),
        server_url,
        path,
    )
    .await
}

async fn patch_workspace(
    client: &reqwest::Client,
    server_url: &str,
    path: &str,
    body: serde_json::Value,
) -> anyhow::Result<WorkspaceDto> {
    request(
        client.patch(format!("{server_url}{path}")).json(&body),
        server_url,
        path,
    )
    .await
}

async fn request<T: serde::de::DeserializeOwned>(
    builder: reqwest::RequestBuilder,
    server_url: &str,
    path: &str,
) -> anyhow::Result<T> {
    let url = format!("{server_url}{path}");
    let res = builder
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(surface_http_error(status, &body));
    }
    serde_json::from_str(&body).with_context(|| format!("unexpected response from {url}: not JSON"))
}

fn surface_http_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
        let msg = val.get("error").and_then(|v| v.as_str()).unwrap_or("");
        let code = val.get("code").and_then(|v| v.as_str());
        if let Some(code) = code {
            return anyhow::anyhow!("{}: {}", code.to_lowercase(), msg);
        }
        if !msg.is_empty() {
            return anyhow::anyhow!("{status}: {msg}");
        }
    }
    anyhow::anyhow!("{status}: {body}")
}

fn print_workspace(workspace: &WorkspaceDto) {
    let marker = if workspace.active { "*" } else { " " };
    println!(
        "{marker} {} ({}) [{}] id={}",
        workspace.name, workspace.slug, workspace.mode, workspace.id
    );
    println!(
        "  channels={} agents={} humans={}",
        workspace.channel_count, workspace.agent_count, workspace.human_count
    );
}
