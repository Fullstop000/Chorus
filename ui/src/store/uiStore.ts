import { create } from 'zustand'
import type { AgentInfo, ChannelInfo } from '../data'
import type { InboxState } from './inbox'
import { createInboxState } from './inbox'

export type ActiveTab = 'chat' | 'tasks' | 'workspace' | 'activity' | 'profile'

/** localStorage key for persisted display preferences. */
const PREFS_KEY = 'chorus:ui-prefs:v1'

interface PersistedPrefs {
  showConversationIds: boolean
}

function readPersistedPrefs(): PersistedPrefs {
  if (typeof localStorage === 'undefined') return { showConversationIds: false }
  try {
    const raw = localStorage.getItem(PREFS_KEY)
    if (!raw) return { showConversationIds: false }
    const parsed = JSON.parse(raw) as Partial<PersistedPrefs>
    return { showConversationIds: !!parsed.showConversationIds }
  } catch {
    return { showConversationIds: false }
  }
}

function writePersistedPrefs(prefs: PersistedPrefs): void {
  if (typeof localStorage === 'undefined') return
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(prefs))
  } catch {
    // Storage quota or disabled — the in-memory value still applies for this session.
  }
}

export interface ToastEntry {
  id: string
  message: string
  level: 'error' | 'warning' | 'info'
}

/**
 * Identifies a task currently rendered in the task-detail view.
 * Parent channel id/slug are carried along so breadcrumbs and data
 * fetching don't have to re-derive them from the tasks board.
 * `returnToTab` captures which tab the user was on when they opened the
 * detail, so the back button can restore that context rather than dumping
 * everyone on Tasks.
 */
export interface TaskDetailTarget {
  parentChannelId: string
  parentSlug: string
  taskNumber: number
  returnToTab?: ActiveTab
}

interface UIState {
  /**
   * Logged-in human's display name, set once after `/api/whoami` resolves.
   * Use `currentUserId` for identity-keyed comparisons; `currentUser` is for
   * display, label, and DM-name composition only.
   */
  currentUser: string
  /**
   * Logged-in human's stable id (the canonical identity for the local
   * session). Empty string until `/api/whoami` resolves.
   */
  currentUserId: string
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
  /** Whether the full-page Settings view is open */
  showSettings: boolean
  /** Whether the Decision Inbox view is open */
  showDecisions: boolean
  /** When non-null, MainPanel renders the task-detail view for this task */
  currentTaskDetail: TaskDetailTarget | null
  /**
   * Display preference: when true, sidebar channel/agent rows show their
   * underlying UUID as a trailing caption. Off by default — UUIDs are
   * routing identifiers, not user content. Persisted to localStorage.
   */
  showConversationIds: boolean
}

interface UIActions {
  /** Set the local human identity (id + name) after /api/whoami resolves. */
  setCurrentUser: (identity: { id: string; name: string }) => void
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
  setShowSettings: (show: boolean) => void
  setShowDecisions: (show: boolean) => void
  setCurrentTaskDetail: (target: TaskDetailTarget | null) => void
  setShowConversationIds: (show: boolean) => void
}

export type UIStore = UIState & UIActions

const initialState: UIState = {
  currentUser: '',
  currentUserId: '',
  currentChannel: null,
  currentAgent: null,
  activeTab: 'chat',
  inboxState: createInboxState(),
  shellBootstrapped: false,
  toasts: [],
  showSettings: false,
  showDecisions: false,
  currentTaskDetail: null,
  showConversationIds: readPersistedPrefs().showConversationIds,
}

export const useStore = create<UIStore>((set) => ({
  ...initialState,

  setCurrentUser: ({ id, name }) =>
    set({ currentUser: name, currentUserId: id }),

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
              // Selecting an agent always exits any open task-detail view —
              // the detail is scoped to a channel, so agent navigation takes
              // us out of it. Without this clear, MainPanel keeps rendering
              // TaskDetail because currentTaskDetail outranks currentAgent.
              currentTaskDetail: null,
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
      // Leaving the parent channel discards any open task-detail view;
      // the detail belongs to a specific parent and can't survive navigation.
      currentTaskDetail:
        channel && state.currentTaskDetail &&
          state.currentTaskDetail.parentChannelId === channel.id
          ? state.currentTaskDetail
          : null,
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
      currentTaskDetail: null,
    }),

  pushToast: (entry: ToastEntry) =>
    set((state) => ({ toasts: [...state.toasts, entry] })),

  dismissToast: (id: string) =>
    set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) })),

  setShowSettings: (showSettings: boolean) => set({ showSettings }),

  setShowDecisions: (showDecisions: boolean) => set({ showDecisions }),

  setCurrentTaskDetail: (currentTaskDetail: TaskDetailTarget | null) =>
    set({ currentTaskDetail }),

  setShowConversationIds: (showConversationIds: boolean) => {
    writePersistedPrefs({ showConversationIds })
    set({ showConversationIds })
  },
}))

export function pushErrorToast(err: unknown) {
  const message = err instanceof Error ? err.message : String(err)
  useStore.getState().pushToast({
    id: crypto.randomUUID(),
    message,
    level: 'error',
  })
}
