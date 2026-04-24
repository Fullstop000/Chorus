//! Assemble [`super::dto`] payloads from [`crate::store::Store`] records.

use anyhow::Result;

use crate::store::channels::ChannelListParams;
use crate::store::Store;

use super::dto::{AgentInfo, ChannelInfo, HumanInfo, ServerInfo, UiShellInfo};

/// Map filtered channels to UI channel rows (membership + read-only flags).
pub fn channel_infos_for(
    store: &Store,
    params: &ChannelListParams<'_>,
) -> Result<Vec<ChannelInfo>> {
    let channels = store.get_channels_by_params(params)?;
    let mut out = Vec::with_capacity(channels.len());
    for channel in channels {
        let joined = match params.for_member {
            Some(member) => store.channel_member_exists(&channel.id, member)?,
            None => false,
        };
        out.push((&channel, joined).into());
    }
    Ok(out)
}

pub fn build_ui_shell_info(store: &Store) -> Result<UiShellInfo> {
    build_ui_shell_info_for_workspace(store, None)
}

pub fn build_ui_shell_info_for_workspace(
    store: &Store,
    workspace_id: Option<&str>,
) -> Result<UiShellInfo> {
    let mut system_channels = channel_infos_for(
        store,
        &ChannelListParams {
            workspace_id,
            include_system: true,
            ..ChannelListParams::default()
        },
    )?;
    system_channels.retain(|ch| ch.channel_type.as_deref() == Some("system"));
    for ch in &mut system_channels {
        ch.joined = true;
    }
    system_channels.sort_by_key(|ch| match ch.name.as_str() {
        name if name == Store::DEFAULT_SYSTEM_CHANNEL => 0,
        _ => 1,
    });

    let humans: Vec<HumanInfo> = store
        .get_humans()?
        .into_iter()
        .map(HumanInfo::from)
        .collect();

    Ok(UiShellInfo {
        system_channels,
        humans,
    })
}

/// Build the agent-scoped workspace snapshot served from the historical
/// `/internal/agent/{agent_id}/server` endpoint.
pub fn build_server_info(store: &Store, for_agent: &str) -> Result<ServerInfo> {
    build_server_info_for_workspace(store, for_agent, None)
}

pub fn build_server_info_for_workspace(
    store: &Store,
    for_agent: &str,
    workspace_id: Option<&str>,
) -> Result<ServerInfo> {
    let channels = channel_infos_for(
        store,
        &ChannelListParams {
            workspace_id,
            for_member: Some(for_agent),
            include_team: true,
            ..ChannelListParams::default()
        },
    )?;
    let agents: Vec<AgentInfo> = store
        .get_agents_for_workspace(workspace_id)?
        .iter()
        .map(AgentInfo::from)
        .collect();
    let shell = build_ui_shell_info_for_workspace(store, workspace_id)?;
    Ok(ServerInfo {
        channels,
        system_channels: shell.system_channels,
        agents,
        humans: shell.humans,
    })
}
