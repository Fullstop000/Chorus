//! `chorus bridge` — remote runtime that connects to a platform over WebSocket.
//!
//! Phase 3 client side. Two-process Chorus: one process runs `chorus serve`
//! as the platform (HTTP/WS API + DB), this process runs the agent runtime
//! and tunnels lifecycle + chat over `/api/bridge/ws`. Local agents talk to
//! an embedded MCP bridge on a loopback port; that bridge proxies tool-calls
//! back to the platform's HTTP API.
//!
//! High level loop:
//!   1. Open `data_dir/bridge.db` (local SQLite for synced agent records).
//!   2. Bind embedded MCP bridge on `bridge_listen` pointed at `platform_http`.
//!   3. Construct `AgentManager` with `bridge_endpoint_override` = local MCP.
//!   4. Dial `platform_ws`, send `bridge.hello`, await `bridge.target`.
//!   5. Reconcile target → local store → AgentManager start/stop.
//!   6. Push `agent.state` upstream on transitions.
//!   7. On `chat.message.received`, wake the agent locally and ack.
//!   8. Reconnect on drop with capped backoff.

use std::sync::Arc;

pub async fn run(
    platform_ws: String,
    platform_http: String,
    token: Option<String>,
    machine_id: String,
    data_dir_str: String,
    bridge_listen: String,
) -> anyhow::Result<()> {
    use chorus::bridge_client;

    let data_dir = std::path::PathBuf::from(&data_dir_str);
    let data_subdir = data_dir.join("data");
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&data_subdir)?;
    std::fs::create_dir_all(&agents_dir)?;

    let db_path = data_subdir.join("chorus-bridge.db");
    let store = Arc::new(chorus::store::Store::open(db_path.to_str().unwrap())?);

    let cfg = bridge_client::BridgeClientConfig {
        platform_ws,
        platform_http,
        token,
        machine_id,
        bridge_listen,
        agents_dir,
        store,
    };

    bridge_client::run_bridge_client(cfg).await
}
