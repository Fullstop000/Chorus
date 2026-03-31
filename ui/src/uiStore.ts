import { create } from 'zustand'
import type { HistoryMessage, ThreadInboxEntry } from './types'
import type { InboxState, ReadCursorAckPayload } from './inbox'
import { createInboxState, mergeReadCursorAckIntoInboxState } from './inbox'

export type ActiveTab = 'chat' | 'threads' | 'tasks' | 'workspace' | 'activity' | 'profile'

interface UIState {
  currentUser: string
  // Store agent name only; derive AgentInfo from the agents query in AppProvider
  selectedAgentName: string | null
  selectedChannel: string | null
  selectedChannelId: string | null
  activeTab: ActiveTab
  openThreadMsg: HistoryMessage | null
  inboxState: InboxState
  conversationThreads: Record<string, ThreadInboxEntry[]>
  shellBootstrapped: boolean
}

interface UIActions {
  setCurrentUser: (user: string) => void
  setSelectedAgentName: (name: string | null) => void
  setSelectedChannel: (ch: string | null, channelId?: string | null) => void
  setActiveTab: (tab: ActiveTab) => void
  setOpenThreadMsg: (msg: HistoryMessage | null) => void
  applyReadCursorAck: (ack: ReadCursorAckPayload) => void
  updateInboxState: (updater: (current: InboxState) => InboxState) => void
  setConversationThreads: (conversationId: string, threads: ThreadInboxEntry[]) => void
  setShellBootstrapped: (value: boolean) => void
  resetUserSession: () => void
}

export type UIStore = UIState & UIActions

const initialState: UIState = {
  currentUser: '',
  selectedAgentName: null,
  selectedChannel: null,
  selectedChannelId: null,
  activeTab: 'chat',
  openThreadMsg: null,
  inboxState: createInboxState(),
  conversationThreads: {},
  shellBootstrapped: false,
}

export const useUIStore = create<UIStore>((set) => ({
  ...initialState,

  setCurrentUser: (currentUser) => set({ currentUser }),

  setSelectedAgentName: (name) =>
    set({
      selectedAgentName: name,
      openThreadMsg: null,
      ...(name ? { selectedChannel: null, selectedChannelId: null, activeTab: 'chat' } : {}),
    }),

  setSelectedChannel: (ch, channelId) =>
    set((state) => ({
      selectedChannel: ch,
      selectedChannelId: ch ? (channelId ?? null) : null,
      openThreadMsg: null,
      selectedAgentName: ch ? null : state.selectedAgentName,
      activeTab:
        ch &&
        (state.activeTab === 'workspace' ||
          state.activeTab === 'activity' ||
          state.activeTab === 'profile')
          ? 'chat'
          : state.activeTab,
    })),

  setActiveTab: (activeTab) => set({ activeTab }),

  setOpenThreadMsg: (openThreadMsg) => set({ openThreadMsg }),

  applyReadCursorAck: (ack) =>
    set((state) => ({
      inboxState: mergeReadCursorAckIntoInboxState(state.inboxState, ack),
    })),

  updateInboxState: (updater) =>
    set((state) => ({ inboxState: updater(state.inboxState) })),

  setConversationThreads: (conversationId, threads) =>
    set((state) => ({
      conversationThreads: { ...state.conversationThreads, [conversationId]: threads },
    })),

  setShellBootstrapped: (shellBootstrapped) => set({ shellBootstrapped }),

  resetUserSession: () =>
    set({
      selectedAgentName: null,
      selectedChannel: null,
      selectedChannelId: null,
      activeTab: 'chat',
      openThreadMsg: null,
      inboxState: createInboxState(),
      conversationThreads: {},
      shellBootstrapped: false,
    }),
}))
