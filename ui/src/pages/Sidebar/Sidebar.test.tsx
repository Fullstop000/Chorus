import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import type { ChannelInfo } from '../../data'

const generalChannel: ChannelInfo = {
  id: '11111111-1111-1111-1111-111111111111',
  name: 'general',
  joined: true,
  channel_type: 'channel',
}

const state = {
  currentUser: 'zht',
  currentUserId: 'zht',
  currentChannel: generalChannel as ChannelInfo | null,
  currentAgent: null,
  showSettings: false,
  showConversationIds: false,
  setCurrentChannel: vi.fn(),
  setCurrentAgent: vi.fn(),
  setShowSettings: vi.fn(),
}

vi.mock('../../store', () => {
  const useStore = ((selector?: (s: typeof state) => unknown) =>
    selector ? selector(state) : state) as unknown as {
    (): typeof state
    <T>(selector: (s: typeof state) => T): T
  }
  return { useStore }
})

vi.mock('../../hooks/data', () => ({
  useAgents: () => [],
  useChannels: () => ({
    allChannels: [generalChannel],
    channels: [generalChannel],
    systemChannels: [],
    dmChannels: [],
  }),
  useHumans: () => [{ id: 'zht', name: 'zht' }],
  useInbox: () => ({
    getConversationUnread: () => 0,
    getAgentUnread: () => 0,
    getAgentConversationId: () => null,
  }),
  useRefresh: () => ({
    refreshChannels: vi.fn(),
    refreshAgents: vi.fn(),
    refreshTeams: vi.fn(),
  }),
  useWorkspaces: () => ({
    workspaces: [
      {
        id: 'ws-1',
        name: 'Chorus Local',
        slug: 'chorus-local',
        mode: 'local_only',
        created_by_human: 'zht',
        created_at: '2026-04-25T00:00:00Z',
        active: true,
        channel_count: 0,
        agent_count: 0,
        human_count: 1,
      },
    ],
    isLoading: false,
    error: null,
  }),
}))

vi.mock('../../components/agents/CreateAgentModal', () => ({
  CreateAgentModal: () => null,
}))

vi.mock('../../components/channels/CreateChannelModal', () => ({
  CreateChannelModal: () => null,
}))

vi.mock('../../components/channels/EditChannelModal', () => ({
  DeleteChannelModal: () => null,
  EditChannelModal: () => null,
}))

const { Sidebar } = await import('./Sidebar')

describe('Sidebar workspace header', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders the active workspace in the sidebar header', () => {
    const html = renderToStaticMarkup(
      <MemoryRouter initialEntries={['/c/general']}>
        <Sidebar />
      </MemoryRouter>,
    )

    expect(html).toContain('current: Chorus Local')
  })
})
