//! `chorus channel create <name>` — stub; HTTP logic lands in a later task.

pub async fn run(
    _name: String,
    _description: Option<String>,
    _server_url: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("not yet implemented")
}
