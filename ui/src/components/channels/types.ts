// ── Channels, members, teams, server channel list ──

export interface ChannelInfo {
  id?: string
  name: string
  description?: string
  joined: boolean
  channel_type?: 'channel' | 'dm' | 'system' | 'team'
  read_only?: boolean
}

export interface HumanInfo {
  name: string
}

export interface ChannelMemberInfo {
  memberName: string
  memberType: 'human' | 'agent'
  displayName?: string
}

export interface ChannelMembersResponse {
  channelId: string
  memberCount: number
  members: ChannelMemberInfo[]
}

export interface ResolveChannelResponse {
  channelId: string
  channelName?: string
}

export interface Team {
  id: string
  name: string
  display_name: string
  channel_id?: string | null
  collaboration_model: 'leader_operators' | 'swarm'
  leader_agent_name?: string | null
  created_at: string
}

export interface TeamMember {
  team_id: string
  member_name: string
  member_type: 'agent' | 'human'
  member_id: string
  role: string
  joined_at: string
}

export interface TeamResponse {
  team: Team
  members: TeamMember[]
}

export interface ServerInfo {
  /** System-managed channels (e.g. #all). Shown separately. */
  system_channels: ChannelInfo[]
  humans: HumanInfo[]
}

export interface WhoamiResponse {
  username: string
}
