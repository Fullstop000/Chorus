import { create } from 'zustand'
import type { AgentInfo, ChannelInfo, HistoryMessage, ThreadInboxEntry } from '../data'
import type { InboxState, ReadCursorAckPayload } from '../inbox'
import { createInboxState, mergeReadCursorAckIntoInboxState } from '../inbox'

export type ActiveTab = 'chat' | 'threads' | 'tasks' | 'workspace' | 'activity' | 'profile'

interface UIState {
  currentUser: string
  currentChannel: ChannelInfo | null
  currentAgent: AgentInfo | null
  activeTab: ActiveTab
  openThreadMsg: HistoryMessage | null
  inboxState: InboxState
  conversationThreads: Record<string, ThreadInboxEntry[]>
  shellBootstrapped: boolean
}

interface UIActions {
  setCurrentUser: (user: string) => void
  setCurrentChannel: (channel: ChannelInfo | null) => void
  setCurrentAgent: (agent: AgentInfo | null) => void
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
  currentChannel: null,
  currentAgent: null,
  activeTab: 'chat',
  openThreadMsg: null,
  inboxState: createInboxState(),
  conversationThreads: {},
  shellBootstrapped: false,
}

export const useStore = create<UIStore>((set) => ({
  ...initialState,

  setCurrentUser: (currentUser: string) => set({ currentUser }),

  setCurrentAgent: (agent: AgentInfo | null) =>
    set({
      currentAgent: agent,
      openThreadMsg: null,
      ...(agent ? { currentChannel: null, activeTab: 'chat' as const } : {}),
    }),

  setCurrentChannel: (channel: ChannelInfo | null) =>
    set((state) => ({
      currentChannel: channel,
      openThreadMsg: null,
      currentAgent: channel ? null : state.currentAgent,
      activeTab:
        channel &&
        (state.activeTab === 'workspace' ||
          state.activeTab === 'activity' ||
          state.activeTab === 'profile')
          ? 'chat'
          : state.activeTab,
    })),

  setActiveTab: (activeTab: ActiveTab) => set({ activeTab }),

  setOpenThreadMsg: (openThreadMsg: HistoryMessage | null) => set({ openThreadMsg }),

  applyReadCursorAck: (ack: ReadCursorAckPayload) =>
    set((state) => ({
      inboxState: mergeReadCursorAckIntoInboxState(state.inboxState, ack),
    })),

  updateInboxState: (updater: (current: InboxState) => InboxState) =>
    set((state) => ({ inboxState: updater(state.inboxState) })),

  setConversationThreads: (conversationId: string, threads: ThreadInboxEntry[]) =>
    set((state) => ({
      conversationThreads: { ...state.conversationThreads, [conversationId]: threads },
    })),

  setShellBootstrapped: (shellBootstrapped: boolean) => set({ shellBootstrapped }),

  resetUserSession: () =>
    set({
      currentAgent: null,
      currentChannel: null,
      activeTab: 'chat',
      openThreadMsg: null,
      inboxState: createInboxState(),
      conversationThreads: {},
      shellBootstrapped: false,
    }),
}))
