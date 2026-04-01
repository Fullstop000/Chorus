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

  setCurrentUser: (currentUser: string) => set({ currentUser }),

  setSelectedAgentName: (name: string | null) =>
    set({
      selectedAgentName: name,
      openThreadMsg: null,
      ...(name ? { selectedChannel: null, selectedChannelId: null, activeTab: 'chat' } : {}),
    }),

  setSelectedChannel: (ch: string | null, channelId?: string | null) =>
    set((state: UIState) => ({
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

  setActiveTab: (activeTab: ActiveTab) => set({ activeTab }),

  setOpenThreadMsg: (openThreadMsg: HistoryMessage | null) => set({ openThreadMsg }),

  applyReadCursorAck: (ack: ReadCursorAckPayload) =>
    set((state: UIState) => ({
      inboxState: mergeReadCursorAckIntoInboxState(state.inboxState, ack),
    })),

  updateInboxState: (updater: (current: InboxState) => InboxState) =>
    set((state: UIState) => ({ inboxState: updater(state.inboxState) })),

  setConversationThreads: (conversationId: string, threads: ThreadInboxEntry[]) =>
    set((state: UIState) => ({
      conversationThreads: { ...state.conversationThreads, [conversationId]: threads },
    })),

  setShellBootstrapped: (shellBootstrapped: boolean) => set({ shellBootstrapped }),

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
