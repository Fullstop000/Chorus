import { create } from 'zustand'
import type { AgentInfo, ChannelInfo } from '../data'
import type { InboxState } from './inbox'
import { createInboxState } from './inbox'

export type ActiveTab = 'chat' | 'tasks' | 'workspace' | 'activity' | 'profile'

export interface ToastEntry {
  id: string
  message: string
  level: 'error' | 'warning' | 'info'
}

interface UIState {
  /** Logged-in username, set once after /api/whoami resolves */
  currentUser: string
  /** Currently selected sidebar channel (team/dm/system); null when an agent is selected instead */
  currentChannel: ChannelInfo | null
  /** Currently selected agent profile; null when a channel is selected */
  currentAgent: AgentInfo | null
  /** Which top-level tab the MainPanel is showing */
  activeTab: ActiveTab
  /** Unread / read-cursor state for every inbox conversation (DMs + channels) */
  inboxState: InboxState
  /** True once the initial whoami + channels + inbox bootstrap has completed; gates autoSelectChannel */
  shellBootstrapped: boolean
  /** Global toast notifications */
  toasts: ToastEntry[]
}

interface UIActions {
  setCurrentUser: (user: string) => void
  setCurrentChannel: (channel: ChannelInfo | null) => void
  setCurrentAgent: (agent: AgentInfo | null) => void
  setActiveTab: (tab: ActiveTab) => void
  /** Bulk-replace inboxState (used by realtime subscription on reconnect) */
  updateInboxState: (updater: (current: InboxState) => InboxState) => void
  /** Optimistically bump latestSeq for a conversation (used by realtime append) */
  advanceConversationLatestSeq: (conversationId: string, seq: number) => void
  /** Optimistically advance lastReadSeq for a conversation (used when messages are viewed) */
  advanceConversationLastReadSeq: (conversationId: string, seq: number) => void
  setShellBootstrapped: (value: boolean) => void
  /** Clear all selection state back to defaults (used on logout / session reset) */
  resetUserSession: () => void
  pushToast: (entry: ToastEntry) => void
  dismissToast: (id: string) => void
}

export type UIStore = UIState & UIActions

const initialState: UIState = {
  currentUser: '',
  currentChannel: null,
  currentAgent: null,
  activeTab: 'chat',
  inboxState: createInboxState(),
  shellBootstrapped: false,
  toasts: [],
}

export const useStore = create<UIStore>((set) => ({
  ...initialState,

  setCurrentUser: (currentUser: string) => set({ currentUser }),

  setCurrentAgent: (agent: AgentInfo | null) =>
    set((state) => {
      const isSameAgent =
        !!agent &&
        !!state.currentAgent &&
        (state.currentAgent.id === agent.id || state.currentAgent.name === agent.name)

      return {
        currentAgent: agent,
        ...(agent
          ? {
              currentChannel: null,
              activeTab: isSameAgent ? state.activeTab : ('chat' as const),
            }
          : {}),
      }
    }),

  setCurrentChannel: (channel: ChannelInfo | null) =>
    set((state) => ({
      currentChannel: channel,
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

  updateInboxState: (updater: (current: InboxState) => InboxState) =>
    set((state) => ({ inboxState: updater(state.inboxState) })),

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

  setShellBootstrapped: (shellBootstrapped: boolean) => set({ shellBootstrapped }),

  resetUserSession: () =>
    set({
      currentAgent: null,
      currentChannel: null,
      activeTab: 'chat',
      inboxState: createInboxState(),
      shellBootstrapped: false,
      toasts: [],
    }),

  pushToast: (entry: ToastEntry) =>
    set((state) => ({ toasts: [...state.toasts, entry] })),

  dismissToast: (id: string) =>
    set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) })),
}))

export function pushErrorToast(err: unknown) {
  const message = err instanceof Error ? err.message : String(err)
  useStore.getState().pushToast({
    id: crypto.randomUUID(),
    message,
    level: 'error',
  })
}
