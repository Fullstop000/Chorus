import { create } from 'zustand'
import type { AgentInfo, ChannelInfo, HistoryMessage, ThreadInboxEntry } from '../data'
import type { InboxState, ReadCursorAckPayload } from '../inbox'
import { createInboxState, mergeReadCursorAckIntoInboxState, threadNotificationKey } from '../inbox'

export type ActiveTab = 'chat' | 'threads' | 'tasks' | 'workspace' | 'activity' | 'profile'

interface UIState {
  /** Logged-in username, set once after /api/whoami resolves */
  currentUser: string
  /** Currently selected sidebar channel (team/dm/system); null when an agent is selected instead */
  currentChannel: ChannelInfo | null
  /** Currently selected agent profile; null when a channel is selected */
  currentAgent: AgentInfo | null
  /** Which top-level tab the MainPanel is showing */
  activeTab: ActiveTab
  /** The message whose thread replies are shown in the ThreadPanel overlay; null = no thread open */
  openThreadMsg: HistoryMessage | null
  /** Unread / read-cursor state for every inbox conversation (DMs + channels) */
  inboxState: InboxState
  /** Thread preview entries keyed by conversationId, used by the ThreadsTab badge count */
  conversationThreads: Record<string, ThreadInboxEntry[]>
  /** True once the initial whoami + channels + inbox bootstrap has completed; gates autoSelectChannel */
  shellBootstrapped: boolean
}

interface UIActions {
  setCurrentUser: (user: string) => void
  setCurrentChannel: (channel: ChannelInfo | null) => void
  setCurrentAgent: (agent: AgentInfo | null) => void
  setActiveTab: (tab: ActiveTab) => void
  setOpenThreadMsg: (msg: HistoryMessage | null) => void
  /** Merge a server-side read-cursor ack into the local inbox state */
  applyReadCursorAck: (ack: ReadCursorAckPayload) => void
  /** Bulk-replace inboxState (used by realtime subscription on reconnect) */
  updateInboxState: (updater: (current: InboxState) => InboxState) => void
  setConversationThreads: (conversationId: string, threads: ThreadInboxEntry[]) => void
  /** Optimistically bump latestSeq for a conversation (used by realtime append) */
  advanceConversationLatestSeq: (conversationId: string, seq: number) => void
  /** Optimistically advance lastReadSeq for a conversation (used when messages are viewed) */
  advanceConversationLastReadSeq: (conversationId: string, seq: number) => void
  /** Optimistically advance lastReadSeq for a thread (used when thread replies are viewed) */
  advanceThreadLastReadSeq: (conversationId: string, threadParentId: string, seq: number) => void
  setShellBootstrapped: (value: boolean) => void
  /** Clear all selection state back to defaults (used on logout / session reset) */
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

  advanceConversationLatestSeq: (conversationId: string, seq: number) =>
    set((state) => {
      const conv = state.inboxState.conversations[conversationId]
      if (!conv || seq <= conv.latestSeq) return state
      return {
        inboxState: {
          ...state.inboxState,
          conversations: {
            ...state.inboxState.conversations,
            [conversationId]: { ...conv, latestSeq: seq },
          },
        },
      }
    }),

  advanceConversationLastReadSeq: (conversationId: string, seq: number) =>
    set((state) => {
      const conv = state.inboxState.conversations[conversationId]
      if (!conv || seq <= conv.lastReadSeq) return state
      return {
        inboxState: {
          ...state.inboxState,
          conversations: {
            ...state.inboxState.conversations,
            [conversationId]: { ...conv, lastReadSeq: seq },
          },
        },
      }
    }),

  advanceThreadLastReadSeq: (conversationId: string, threadParentId: string, seq: number) =>
    set((state) => {
      const key = threadNotificationKey(conversationId, threadParentId)
      const thread = state.inboxState.threads[key]
      if (!thread || seq <= thread.lastReadSeq) return state
      return {
        inboxState: {
          ...state.inboxState,
          threads: {
            ...state.inboxState.threads,
            [key]: { ...thread, lastReadSeq: seq },
          },
        },
      }
    }),

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
