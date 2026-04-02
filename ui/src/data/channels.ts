import { get, post, patch, del } from './client'
import { queryString } from './common'
import { queryOptions } from '@tanstack/react-query'
import type {
  ChannelInfo,
  ChannelMembersResponse,
  HumanInfo,
  ResolveChannelResponse,
  ServerInfo,
  WhoamiResponse,
} from '../types'

export type {
  ChannelInfo,
  ChannelMemberInfo,
  ChannelMembersResponse,
  HumanInfo,
  ResolveChannelResponse,
  Team,
  TeamMember,
  TeamResponse,
  ServerInfo,
  WhoamiResponse,
} from '../types'

export function listChannels(params?: {
  member?: string
  includeArchived?: boolean
  includeDm?: boolean
  includeSystem?: boolean
  includeTeam?: boolean
}): Promise<ChannelInfo[]> {
  return get(`/api/channels${queryString(params ?? {})}`)
}

export function createChannel(payload: {
  name: string
  description: string
}): Promise<{ id: string; name: string }> {
  return post('/api/channels', payload)
}

export function updateChannel(
  channelId: string,
  payload: { name: string; description: string }
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
  return post(`/api/channels/${encodeURIComponent(channelId)}/members`, { memberName })
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

export const channelQueryKeys = {
  whoami: ['whoami'] as const,
  channels: (user: string) => ['channels', user] as const,
  humans: ['humans'] as const,
} as const

export const whoamiQuery = queryOptions({
  queryKey: channelQueryKeys.whoami,
  queryFn: () => getWhoami().then((r) => r.username),
  staleTime: Infinity,
})

export const channelsQuery = (currentUser: string) =>
  queryOptions({
    queryKey: channelQueryKeys.channels(currentUser),
    queryFn: () => listChannels({ member: currentUser, includeDm: true, includeSystem: true }),
    enabled: !!currentUser,
    select: sliceChannels,
  })

export const humansQuery = (currentUser: string) =>
  queryOptions({
    queryKey: channelQueryKeys.humans,
    queryFn: listHumans,
    enabled: !!currentUser,
  })
