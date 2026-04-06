import { create } from 'zustand'
import type { AgentInfo, ChannelInfo, HistoryMessage, ThreadInboxEntry } from '../data'
import type { InboxState, ReadCursorAckPayload } from '../inbox'
import { createInboxState, mergeReadCursorAckIntoInboxState } from '../inbox'

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
  /** Per-conversation unread message IDs collected from streaming events */
  unreadMessageIds: Record<string, Set<string>>
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
  /** Add a message ID to the unread set for a conversation */
  addUnreadMessageId: (conversationId: string, messageId: string) => void
  /** Remove a message ID from the unread set for a conversation (called when rendered/seen) */
  markUnreadAsSeen: (conversationId: string, messageId: string, messageContent?: string) => void
  /** Clear all unread IDs for a conversation (called on scroll-to-bottom) */
  clearAllUnread: (conversationId: string) => void
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
  unreadMessageIds: {},
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

  addUnreadMessageId: (conversationId: string, messageId: string) =>
    set((state) => {
      const prev = state.unreadMessageIds[conversationId] ?? new Set<string>()
      return {
        unreadMessageIds: {
          ...state.unreadMessageIds,
          [conversationId]: new Set(prev).add(messageId),
        },
      }
    }),

  markUnreadAsSeen: (conversationId: string, messageId: string, messageContent?: string) =>
    set((state) => {
      const prev = state.unreadMessageIds[conversationId]
      if (!prev || !prev.has(messageId)) return state
      const convName =
        state.currentChannel?.name ??
        state.currentAgent?.display_name ??
        state.currentAgent?.name ??
        conversationId
      const next = new Set(prev)
      next.delete(messageId)
      console.log(
        `[markSeen] conversation=${convName} (${conversationId}) agent=${state.currentAgent?.name} msg=${messageId} content=${messageContent?.slice(0, 20) ?? '(unknown)'} unreadCnt: ${prev.size} → ${next.size}`
      )
      return {
        unreadMessageIds: { ...state.unreadMessageIds, [conversationId]: next },
      }
    }),

  clearAllUnread: (conversationId: string) =>
    set((state) => {
      console.log('clearAllUnread', conversationId, 'unreadMessageIDs:', state.unreadMessageIds[conversationId])
      if (!state.unreadMessageIds[conversationId]) return state
      const convName =
        state.currentChannel?.name ??
        state.currentAgent?.display_name ??
        state.currentAgent?.name ??
        conversationId
      console.log(
        `[clearAllUnread] conversation=${convName} (${conversationId}) cleared ${state.unreadMessageIds[conversationId].size} messages`
      )
      return {
        unreadMessageIds: { ...state.unreadMessageIds, [conversationId]: new Set() },
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
      unreadMessageIds: {},
      shellBootstrapped: false,
    }),
}))
