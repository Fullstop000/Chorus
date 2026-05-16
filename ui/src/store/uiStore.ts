import { create } from 'zustand'
import type { InboxState } from './inbox'
import { createInboxState } from './inbox'

/**
 * Which subview is active. Derived from the URL via `useRouteSubject`;
 * kept here as a type for components that still reason in terms of tabs
 * (e.g. `MainPanel`'s render cascade).
 */
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
  /** Unread / read-cursor state for every inbox conversation (DMs + channels) */
  inboxState: InboxState
  /** True once the initial whoami + channels + inbox bootstrap has completed; gates RootRedirect */
  shellBootstrapped: boolean
  /** Global toast notifications */
  toasts: ToastEntry[]
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
  /** Bulk-replace inboxState (used by realtime subscription on reconnect) */
  updateInboxState: (updater: (current: InboxState) => InboxState) => void
  /** Optimistically bump latestSeq for a conversation (used by realtime append) */
  advanceConversationLatestSeq: (conversationId: string, seq: number) => void
  /** Optimistically advance lastReadSeq for a conversation (used when messages are viewed) */
  advanceConversationLastReadSeq: (conversationId: string, seq: number) => void
  setShellBootstrapped: (value: boolean) => void
  /** Clear inbox state back to defaults (used on logout / session reset). Navigation state lives in the URL, not the store. */
  resetUserSession: () => void
  pushToast: (entry: ToastEntry) => void
  dismissToast: (id: string) => void
  setShowConversationIds: (show: boolean) => void
}

export type UIStore = UIState & UIActions

const initialState: UIState = {
  currentUser: '',
  currentUserId: '',
  inboxState: createInboxState(),
  shellBootstrapped: false,
  toasts: [],
  showConversationIds: readPersistedPrefs().showConversationIds,
}

export const useStore = create<UIStore>((set) => ({
  ...initialState,

  setCurrentUser: ({ id, name }) =>
    set({ currentUser: name, currentUserId: id }),

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
      inboxState: createInboxState(),
      shellBootstrapped: false,
      toasts: [],
    }),

  pushToast: (entry: ToastEntry) =>
    set((state) => ({ toasts: [...state.toasts, entry] })),

  dismissToast: (id: string) =>
    set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) })),

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
