import { useMatch, useParams } from 'react-router-dom'
import { useStore } from '../store/uiStore'
import { useAgents, useChannels } from './data'
import type { AgentInfo, ChannelInfo } from '../data'
import { dmConversationNameForParticipants } from '../data'
import { isSettingsSection, type SettingsSection } from '../lib/routes'

/**
 * What the current URL points at, resolved against the React Query cache.
 *
 * During the migration this is consumed by `UrlToStoreSync` (in App.tsx)
 * which mirrors the resolved subject into the legacy `uiStore` nav fields,
 * letting unchanged components keep reading from the store. After the
 * mirror layer is retired this hook becomes the direct read path.
 */
export type RouteSubject =
  | { kind: 'channel'; channel: ChannelInfo; view: 'chat' | 'tasks' }
  | { kind: 'task'; channel: ChannelInfo; taskNumber: number }
  | { kind: 'dm'; agent: AgentInfo; channel: ChannelInfo | null }
  | { kind: 'agent-tab'; agent: AgentInfo; tab: 'profile' | 'activity' | 'workspace' }
  | { kind: 'inbox' }
  | { kind: 'settings'; section: SettingsSection }
  | { kind: 'root' }
  | { kind: 'unknown' }

function findChannelByName(channels: ChannelInfo[], name: string): ChannelInfo | undefined {
  return channels.find((c) => c.name === name)
}

function findAgentByName(agents: AgentInfo[], name: string): AgentInfo | undefined {
  return agents.find((a) => a.name === name)
}

function findDmChannel(params: {
  humanId: string
  humanDisplayName: string
  agent: AgentInfo
  dmChannels: ChannelInfo[]
}): ChannelInfo | undefined {
  const { humanId, humanDisplayName, agent, dmChannels } = params
  const candidates = new Set<string>()
  if (humanId && agent.id) {
    candidates.add(dmConversationNameForParticipants(humanId, agent.id))
  }
  if (humanDisplayName && agent.name) {
    candidates.add(dmConversationNameForParticipants(humanDisplayName, agent.name))
  }
  return dmChannels.find((c) => candidates.has(c.name))
}

/**
 * Convenience: the channel the user is in for channel-route contexts only.
 * Returns null on DM and agent routes — matches the legacy `currentChannel`
 * field semantics, where DMs were carried via `currentAgent` and
 * `currentChannel` was null.
 */
export function useCurrentChannel(): ChannelInfo | null {
  const subject = useRouteSubject()
  if (subject.kind === 'channel' || subject.kind === 'task') return subject.channel
  return null
}

/**
 * Convenience: the agent the user is interacting with on DM and agent-tab
 * routes. Returns null in channel and other contexts — matches legacy
 * `currentAgent` semantics.
 */
export function useCurrentAgent(): AgentInfo | null {
  const subject = useRouteSubject()
  if (subject.kind === 'dm' || subject.kind === 'agent-tab') return subject.agent
  return null
}

export function useRouteSubject(): RouteSubject {
  const params = useParams()
  const channelMatch = useMatch('/c/:channel/*')
  const tasksBoardMatch = useMatch('/c/:channel/tasks')
  const taskDetailMatch = useMatch('/c/:channel/tasks/:n')
  const dmMatch = useMatch('/dm/:agent')
  const agentTabMatch = useMatch('/agent/:agent/:tab')
  const inboxMatch = useMatch('/inbox')
  const settingsMatch = useMatch('/settings/:section')
  const settingsRootMatch = useMatch('/settings')
  const rootMatch = useMatch({ path: '/', end: true })

  const currentUserId = useStore((s) => s.currentUserId)
  const currentUser = useStore((s) => s.currentUser)
  const agents = useAgents()
  const { allChannels, dmChannels } = useChannels()

  if (rootMatch) return { kind: 'root' }
  if (inboxMatch) return { kind: 'inbox' }

  if (settingsMatch) {
    const section = settingsMatch.params.section
    if (section && isSettingsSection(section)) return { kind: 'settings', section }
    return { kind: 'unknown' }
  }
  if (settingsRootMatch) return { kind: 'settings', section: 'profile' }

  if (taskDetailMatch) {
    const name = params.channel
    if (!name) return { kind: 'unknown' }
    const channel = findChannelByName(allChannels, name)
    if (!channel) return { kind: 'unknown' }
    const n = Number(params.n)
    if (!Number.isInteger(n) || n <= 0) return { kind: 'unknown' }
    return { kind: 'task', channel, taskNumber: n }
  }

  if (channelMatch) {
    const name = params.channel
    if (!name) return { kind: 'unknown' }
    const channel = findChannelByName(allChannels, name)
    if (!channel) return { kind: 'unknown' }
    return { kind: 'channel', channel, view: tasksBoardMatch ? 'tasks' : 'chat' }
  }

  if (dmMatch) {
    const name = params.agent
    if (!name) return { kind: 'unknown' }
    const agent = findAgentByName(agents, name)
    if (!agent) return { kind: 'unknown' }
    const channel =
      findDmChannel({
        humanId: currentUserId,
        humanDisplayName: currentUser,
        agent,
        dmChannels,
      }) ?? null
    return { kind: 'dm', agent, channel }
  }

  if (agentTabMatch) {
    const name = params.agent
    const tab = params.tab
    if (!name) return { kind: 'unknown' }
    const agent = findAgentByName(agents, name)
    if (!agent) return { kind: 'unknown' }
    if (tab === 'profile' || tab === 'activity' || tab === 'workspace') {
      return { kind: 'agent-tab', agent, tab }
    }
    return { kind: 'unknown' }
  }

  return { kind: 'unknown' }
}
