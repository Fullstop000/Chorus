# Unread Tracking Refactor: `unreadMessageIds` as Root Truth

**Date:** 2026-04-04
**Goal:** Eliminate per-message server API calls by making `unreadMessageIds` the single source of truth for unread counts. Load inbox data once at bootstrap; no re-fetches on read-cursor or new messages.

## Problem

Three sources of unnecessary server load:

1. **Per-message inbox refresh** (`subscribeInbox` in App.tsx): Every `message.created` WebSocket event triggers `GET /api/conversations/{id}/inbox-notification`. One API call per incoming message.
2. **Read-cursor ack invalidates inbox query**: After POST read-cursor, `applyReadCursorAck` calls `queryClient.invalidateQueries({ queryKey: ["inbox"] })`.
3. **Dual bookkeeping**: Badge count = `inboxState.unreadCount` + `unreadMessageIds.size`. Two systems that must stay synchronized but are updated independently.

## Design

### Architecture

```
                    BOOTSTRAP (once)
                         │
          ┌──────────────┼──────────────┐
          ▼              ▼              ▼
   GET /api/inbox   For each conv     Seed unreadMessageIds
   → inboxState     with unread>0:    with fetched IDs
                     GET /api/conversations/{id}/history?after={lastReadSeq}
                     → collect msg IDs where seq > lastReadSeq
```

```
                    RUNTIME (streaming)
                         │
     message.created ────┤
       │                 │
       ├→ addUnreadMessageId(convId, msgId)    ← add ID to set
       └→ NO inbox notification fetch           ← removed (was culprit #1)

     message renders visible ──┤
       │                       │
       ├→ markUnreadAsSeen()   ← remove ID from set
       └→ debounced read-cursor POST to server
           └→ response: update inboxState metadata only
              (lastReadSeq, latestSeq) — NOT unreadCount
              NO query invalidation            ← removed (was culprit #2)

     scroll-to-bottom ──┤
       │                │
       ├→ clearAllUnread()  ← clear all IDs for conversation
       └→ read-cursor POST  (fire-and-forget)

     Badge count = unreadMessageIds[convId].size   ← single source ✅
```

### State Changes

**`unreadMessageIds` becomes root truth:**
- Type stays `Record<string, Set<string>>`
- Seeded at bootstrap from fetched history (not empty)
- All UI badge/divider logic reads from it exclusively
- No longer combined with `inboxState.unreadCount`

**`inboxState` demoted to metadata-only:**
- Still loaded at bootstrap (for thread state, conversation registry)
- Updated by read-cursor ack responses (for `lastReadSeq`, `latestSeq` tracking only)
- **No longer used for `unreadCount`** in any UI path
- `mergeInboxNotificationRefresh` on message.created events is **removed**

### File-by-File Changes

#### 1. `ui/src/store/uiStore.ts`
- Add `seedUnreadMessageIds(conversationId: string, ids: string[])` action
- `clearAllUnread` and `markUnreadAsSeen` stay unchanged
- Remove or deprecate `applyReadCursorAck` updating `unreadCount` field (keep for seq tracking)

#### 2. `ui/src/hooks/data.ts`
- **`getConversationUnread()`**: Return `unreadMessageIds[conversationId]?.size ?? 0` only. Remove `serverUnread + clientUnread` combination.
- Remove `inboxState` dependency from `useAppInboxSelectors` unread computation.

#### 3. `ui/src/App.tsx`
- **Bootstrap flow** (new): After `inboxQuery.data` resolves, for each conversation where `unreadCount > 0`, call `GET /api/conversations/{id}/history?after={lastReadSeq}` to fetch actual unread message IDs, then call `seedUnreadMessageIds(convId, ids)`.
- **`subscribeInbox`**: Remove the `message.created` handler that calls `getConversationInboxNotification` and `mergeInboxNotificationRefresh`. Keep subscription alive for other event types if needed; otherwise simplify.
- **`applyReadCursorAck`**: Remove `queryClient.invalidateQueries({ queryKey: ["inbox"] })`. Keep `useStore.getState().applyReadCursorAck(ack)` for seq metadata updates only.

#### 4. `ui/src/hooks/useHistory.ts`
- No changes needed. Already calls `addUnreadMessageId` on new messages and dispatches read-cursor via `onReadCursorAck`.

#### 5. `ui/src/components/chat/MessageList.tsx`
- No changes needed. Already reads `unreadIds` prop (sourced from `unreadMessageIds`).

### Server Load Impact

| Before | After |
|--------|-------|
| 1 GET per incoming message (inbox-notification) | 0 |
| Inbox query invalidation on every read-cursor | 0 |
| N+1 GET at bootstrap (fetch unread IDs per conversation) | N+1 GET once at startup only |

N = number of conversations with unread messages. Typically small (1-5). Acceptable one-time cost.

### Edge Cases

- **Conversation with 100+ unread at bootstrap**: History fetch returns paginated results. Use existing pagination params. Cap at reasonable limit (e.g., last 200 messages).
- **Bootstrap race condition**: If streaming events arrive before bootstrap seeding completes, `addUnreadMessageId` adds to the set normally. Seeding then adds remaining IDs. Set deduplication is natural (Set).
- **Tab background**: Read-cursor still fires (existing visibility check in useHistory). Server gets updated regardless of UI state.
- **Reconnect**: Full reconnect already re-runs bootstrap. `resetUserSession` clears everything, then fresh bootstrap reseeds.

### What Stays the Same

- `clearAllUnread` / `markUnreadAsSeen` behavior
- Read-cursor POST timing and debouncing (150ms in useHistory)
- `NewMessageDivider` / `NewMessageBadge` rendering logic
- Thread unread tracking (separate concern, untouched)
