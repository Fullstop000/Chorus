//! `chorus start` — start the server and open the web UI in the default browser.
//!
//! Delegates to [`serve::run`] after optionally spawning a background task
//! that polls `/health` and calls `open::that` once the server is ready.
//! Pass `--no-open` to skip the browser launch (same as the former `serve`).

use super::{default_data_dir, resolve_template_dir, serve};

pub async fn run(
    port: u16,
    data_dir: Option<String>,
    no_open: bool,
    template_dir: Option<String>,
) -> anyhow::Result<()> {
    let data_dir_str = data_dir.unwrap_or_else(default_data_dir);
    let template_dir = resolve_template_dir(&data_dir_str, template_dir);

    if !no_open {
        let url = format!("http://localhost:{port}");
        tokio::spawn(async move {
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(400))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(err = %e, "failed to build health-probe client; skipping browser open");
                    return;
                }
            };
            let health = format!("{url}/health");
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Ok(res) = client.get(&health).send().await {
                    if res.status().is_success() {
                        if let Err(e) = open::that(&url) {
                            tracing::warn!(url = %url, err = %e, "could not open browser");
                        }
                        return;
                    }
                }
            }
            tracing::warn!(
                "server did not respond to /health within budget; skipping browser open"
            );
        });
    }

    serve::run(port, data_dir_str, template_dir, 4321).await
}
