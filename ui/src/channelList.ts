import type { ChannelInfo, Team } from './types'

export function teamToChannelInfo(team: Team): ChannelInfo {
  return {
    id: team.channel_id ?? undefined,
    name: team.name,
    description: team.display_name,
    joined: true,
    channel_type: 'team',
  }
}

export function mergeUserAndTeamChannels(channels: ChannelInfo[], teams: Team[]): ChannelInfo[] {
  const merged = [...channels]
  const seen = new Set(channels.map((channel) => channel.id ?? `name:${channel.name}`))

  for (const team of teams) {
    const byId = team.id
    const byName = `name:${team.name}`
    if (seen.has(byId) || seen.has(byName)) continue
    merged.push(teamToChannelInfo(team))
    seen.add(byId)
    seen.add(byName)
  }

  return merged
}
