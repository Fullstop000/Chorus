import { get, post, patch, del } from './client'
import { queryString } from './common'
import { queryOptions } from '@tanstack/react-query'
import type {
  ListChannelsParams,
  CreateChannelRequest,
  UpdateChannelRequest,
  InviteChannelMemberRequest,
} from './requests'

// ── Types (source of truth) ──

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
  system_channels: ChannelInfo[]
  humans: HumanInfo[]
}

export interface WhoamiResponse {
  username: string
}

// ── API functions ──

export function listChannels(params?: ListChannelsParams): Promise<ChannelInfo[]> {
  return get(`/api/channels${queryString((params ?? {}) as Record<string, string | number | boolean | undefined>)}`)
}

export function createChannel(payload: CreateChannelRequest): Promise<{ id: string; name: string }> {
  return post('/api/channels', payload)
}

export function updateChannel(
  channelId: string,
  payload: UpdateChannelRequest
): Promise<{ id: string; name: string; description?: string | null }> {
  return patch(`/api/channels/${encodeURIComponent(channelId)}`, payload)
}

export function archiveChannel(channelId: string): Promise<{ ok: boolean }> {
  return post(`/api/channels/${encodeURIComponent(channelId)}/archive`)
}

export function deleteChannel(channelId: string): Promise<{ ok: boolean }> {
  return del(`/api/channels/${encodeURIComponent(channelId)}`)
}

export function getChannelMembers(channelId: string): Promise<ChannelMembersResponse> {
  return get(`/api/channels/${encodeURIComponent(channelId)}/members`)
}

export function inviteChannelMember(
  channelId: string,
  memberName: string
): Promise<ChannelMembersResponse> {
  return post(`/api/channels/${encodeURIComponent(channelId)}/members`, { memberName } satisfies InviteChannelMemberRequest)
}

export function ensureDirectMessageConversation(peerName: string): Promise<ChannelInfo> {
  return putDm(peerName)
}

function putDm(peerName: string): Promise<ChannelInfo> {
  return post(`/api/dms/${encodeURIComponent(peerName)}`, undefined, { method: 'PUT' })
}

export function listHumans(): Promise<HumanInfo[]> {
  return get('/api/humans')
}

export function getServerInfo(): Promise<ServerInfo> {
  return get('/api/server-info')
}

export function getWhoami(): Promise<WhoamiResponse> {
  return get('/api/whoami')
}

export function resolveChannel(username: string, target: string): Promise<ResolveChannelResponse> {
  return post(`/internal/agent/${encodeURIComponent(username)}/resolve-channel`, { target })
}

// ── Transforms ──

export type ChannelSlices = {
  allChannels: ChannelInfo[]
  channels: ChannelInfo[]
  systemChannels: ChannelInfo[]
  dmChannels: ChannelInfo[]
}

export function sliceChannels(raw: ChannelInfo[]): ChannelSlices {
  const channels: ChannelInfo[] = []
  const systemChannels: ChannelInfo[] = []
  const dmChannels: ChannelInfo[] = []
  for (const ch of raw) {
    if (ch.channel_type === 'dm') dmChannels.push(ch)
    else if (ch.channel_type === 'system') systemChannels.push(ch)
    else channels.push(ch)
  }
  return { allChannels: raw, channels, systemChannels, dmChannels }
}

// ── Query definitions ──

export const channelQueryKeys = {
  whoami: ['whoami'] as const,
  channels: (user: string) => ['channels', user] as const,
  humans: ['humans'] as const,
  members: (channelId: string) => ['channelMembers', channelId] as const,
} as const

export const whoamiQuery = queryOptions({
  queryKey: channelQueryKeys.whoami,
  queryFn: () => getWhoami().then((r) => r.username),
  staleTime: Infinity,
})

export const channelsQuery = (currentUser: string) =>
  queryOptions({
    queryKey: channelQueryKeys.channels(currentUser),
    queryFn: () => listChannels({ member: currentUser, include_dm: true, include_system: true }),
    enabled: !!currentUser,
    select: sliceChannels,
  })

export const humansQuery = (currentUser: string) =>
  queryOptions({
    queryKey: channelQueryKeys.humans,
    queryFn: listHumans,
    enabled: !!currentUser,
  })

export const channelMembersQuery = (channelId: string | null) =>
  queryOptions({
    queryKey: channelQueryKeys.members(channelId ?? ''),
    queryFn: () => getChannelMembers(channelId!),
    enabled: !!channelId,
  })
