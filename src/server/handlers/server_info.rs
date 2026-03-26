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
    let channels = store.list_channels_for_params(params)?;
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
    let mut system_channels = channel_infos_for(
        store,
        &ChannelListParams {
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
        name if name == Store::SHARED_MEMORY_CHANNEL => 1,
        _ => 2,
    });

    let humans: Vec<HumanInfo> = store
        .list_humans()?
        .into_iter()
        .map(HumanInfo::from)
        .collect();

    Ok(UiShellInfo {
        system_channels,
        humans,
    })
}

pub fn build_server_info(store: &Store, for_agent: &str) -> Result<ServerInfo> {
    let channels = channel_infos_for(
        store,
        &ChannelListParams {
            for_member: Some(for_agent),
            include_team: true,
            ..ChannelListParams::default()
        },
    )?;
    let agents: Vec<AgentInfo> = store.list_agents()?.iter().map(AgentInfo::from).collect();
    let shell = build_ui_shell_info(store)?;
    Ok(ServerInfo {
        channels,
        system_channels: shell.system_channels,
        agents,
        humans: shell.humans,
    })
}
