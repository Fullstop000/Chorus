import { get, post, patch, del, ApiError } from './client'
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
  channel_type?: 'channel' | 'dm' | 'system' | 'team' | 'task'
  parent_channel_id?: string | null
  read_only?: boolean
}

export interface HumanInfo {
  /** Stable human id (canonical identity). */
  id: string
  /** Display / lookup label. */
  name: string
}

export interface SystemInfo {
  data_dir: string
  db_size_bytes: number | null
  config?: ConfigInfo
}

export interface ConfigInfo {
  machine_id?: string
  local_human?: {
    name: string
  }
  agent_template: {
    dir?: string
    default: string
  }
  logs: {
    level: string
    rotation: string
    retention: number
  }
  runtimes: {
    name: string
    binary_path?: string
    acp_adaptor?: string
  }[]
}

export interface LogsResponse {
  lines: string[]
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

/**
 * `/api/whoami` exposes the local human's stable id alongside the display
 * name. Identity-keyed UI logic (e.g. `senderId === me.id`) MUST use `id`;
 * `name` is for display and label-only flows.
 */
export interface WhoamiResponse {
  id: string
  name: string
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

export function ensureDirectMessageConversation(peerId: string): Promise<ChannelInfo> {
  return putDm(peerId)
}

function putDm(peerId: string): Promise<ChannelInfo> {
  return post(`/api/dms/${encodeURIComponent(peerId)}`, undefined, { method: 'PUT' })
}

export function listHumans(): Promise<HumanInfo[]> {
  return get('/api/humans')
}

export function getSystemInfo(): Promise<SystemInfo> {
  return get('/api/system-info')
}

export function getLogs(tail = 200): Promise<LogsResponse> {
  return get(`/api/logs?tail=${tail}`)
}

export function getServerInfo(): Promise<ServerInfo> {
  return get('/api/server-info')
}

/**
 * Resolve the current user. On a fresh browser (no `chorus_sid` cookie
 * yet), `/api/whoami` returns 401; we then mint a cookie via the
 * loopback-only `/api/auth/local-session` endpoint and retry once.
 *
 * Done lazily here rather than eagerly on every page load so repeat
 * visits don't create a new session row per refresh.
 */
export async function getWhoami(): Promise<WhoamiResponse> {
  try {
    return await get<WhoamiResponse>('/api/whoami')
  } catch (err) {
    if (err instanceof ApiError && err.status === 401) {
      await mintLocalSession()
      return await get<WhoamiResponse>('/api/whoami')
    }
    throw err
  }
}

/**
 * Local-mode bootstrap: POST /api/auth/local-session to mint a session
 * cookie. The endpoint is loopback-gated; cloud builds will replace this
 * with a real auth flow.
 */
async function mintLocalSession(): Promise<void> {
  const res = await fetch('/api/auth/local-session', {
    method: 'POST',
    credentials: 'same-origin',
  })
  if (!res.ok) {
    throw new ApiError(
      res.status,
      `failed to bootstrap local session (status ${res.status}); run \`chorus setup\` if you haven't already`
    )
  }
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
  /** React-query cache segment keyed by the local human's stable id. */
  channels: (humanId: string) => ['channels', humanId] as const,
  humans: (humanId: string) => ['humans', humanId] as const,
  members: (channelId: string | null) => ['channelMembers', channelId] as const,
} as const

export const whoamiQuery = queryOptions({
  queryKey: channelQueryKeys.whoami,
  queryFn: () => getWhoami(),
  staleTime: 5 * 60 * 1000,
})

export const channelsQuery = (memberHumanId: string) =>
  queryOptions({
    queryKey: channelQueryKeys.channels(memberHumanId),
    queryFn: () =>
      listChannels({ member: memberHumanId, include_dm: true, include_system: true }),
    enabled: !!memberHumanId,
    select: sliceChannels,
  })

export const humansQuery = (memberHumanId: string) =>
  queryOptions({
    queryKey: channelQueryKeys.humans(memberHumanId),
    queryFn: listHumans,
    enabled: !!memberHumanId,
  })

export const channelMembersQuery = (channelId: string | null) =>
  queryOptions({
    queryKey: channelQueryKeys.members(channelId),
    queryFn: () => getChannelMembers(channelId as string),
    enabled: !!channelId,
  })
