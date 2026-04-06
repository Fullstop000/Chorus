import { useEffect, useRef, useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { MessageItem } from "./MessageItem";
import { NewMessageDivider } from "./NewMessageDivider";
import { NewMessageBadge } from "./NewMessageBadge";
import { useVisibilityTracking } from "../../hooks/useVisibilityTracking";
import { updateReadCursor, historyQueryKeys } from "../../data";
import type { HistoryMessage, HistoryResponse } from "../../data";
import { useStore } from "../../store";
import "./MessageList.css";
import type { RefObject } from "react";

type ScrollMetrics = {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
};

export function isNearBottom(
  { scrollHeight, scrollTop, clientHeight }: ScrollMetrics,
  threshold = 10,
) {
  return scrollHeight - scrollTop - clientHeight < threshold;
}

export function getBottomTransition(
  wasAtBottom: boolean,
  metrics: ScrollMetrics,
  threshold = 10,
) {
  const nowAtBottom = isNearBottom(metrics, threshold);
  if (nowAtBottom && !wasAtBottom) return "entered";
  if (!nowAtBottom && wasAtBottom) return "left";
  return "none";
}

interface MessageListProps {
  // Stable store key for the current channel, DM, or thread.
  targetKey: string;
  // Conversation id for read-cursor tracking.
  conversationId: string | null;
  // Messages rendered in visual order.
  messages: HistoryMessage[];
  // True while history is still loading.
  loading: boolean;
  // Highest sequence number the viewer has already read.
  lastReadSeq: number;
  // Logged-in username for self styling.
  currentUser: string | null;
  // Chooses whether this component owns scrolling or inherits a parent scroller.
  scrollMode?: "internal" | "inherit";
  // Parent scroll container used when scrollMode is inherit.
  externalScrollContainerRef?: RefObject<HTMLDivElement | null>;
  // Optional reply action for each message row.
  onReply?: (message: HistoryMessage) => void;
  // Empty state copy shown when no messages exist.
  emptyLabel?: string;
  // Thread parent id — set when rendering a thread message list.
  threadParentId?: string | null;
}

export function MessageList({
  targetKey,
  conversationId,
  messages,
  loading,
  lastReadSeq,
  currentUser,
  scrollMode = "internal",
  externalScrollContainerRef,
  onReply,
  emptyLabel = "No messages yet. Be the first to say something!",
  threadParentId,
}: MessageListProps) {
  // ── DOM refs ──
  const bottomRef = useRef<HTMLDivElement>(null);
  const internalScrollContainerRef = useRef<HTMLDivElement>(null);
  const firstUnreadAnchorRef = useRef<HTMLDivElement>(null);
  const messageRowRefs = useRef<Map<string, HTMLDivElement>>(new Map());

  // ── Read cursor refs ──
  const lastReadSeqRef = useRef(0);
  const pendingReadSeqRef = useRef<number | null>(null);
  const readCursorTimerRef = useRef<number | null>(null);

  // ── Target tracking refs ──
  const lastTargetRef = useRef<string>("");
  const activeTargetRef = useRef(targetKey);
  activeTargetRef.current = targetKey;

  // ── Store / query ──
  const queryClient = useQueryClient();
  const queryKey = historyQueryKeys.history(
    conversationId ?? "",
    threadParentId ?? null,
  );
  const { advanceConversationLastReadSeq, advanceThreadLastReadSeq } =
    useStore();

  // ── Sync refs with props ──
  useEffect(() => {
    lastReadSeqRef.current = lastReadSeq;
  }, [lastReadSeq]);

  useEffect(() => {
    pendingReadSeqRef.current = null;
    lastReadSeqRef.current = lastReadSeq;
  }, [targetKey]);

  // ── Debounced server flush ──
  // Sends the highest pending read-cursor seq to the server after 150ms of quiet.
  // Guards: stale target, tab hidden, seq already persisted.
  const flushReadCursor = useCallback(async () => {
    readCursorTimerRef.current = null;
    const flushSeq = pendingReadSeqRef.current;
    pendingReadSeqRef.current = null;
    // Nothing new to persist, or target changed while timer was pending.
    if (flushSeq == null || flushSeq <= lastReadSeqRef.current) return;
    if (activeTargetRef.current !== targetKey) return;
    if (document.visibilityState !== "visible") return;
    try {
      await updateReadCursor(
        conversationId!,
        flushSeq,
        threadParentId || undefined,
      );
      // Sync the server-confirmed seq back into the React Query cache.
      queryClient.setQueryData<HistoryResponse | undefined>(
        queryKey,
        (current) =>
          current
            ? {
                ...current,
                last_read_seq: Math.max(current.last_read_seq ?? 0, flushSeq),
              }
            : current,
      );
    } catch (cursorError) {
      console.error("Failed to update read cursor", cursorError);
    }
  }, [conversationId, threadParentId, targetKey, queryClient, queryKey]);

  // ── Visibility callback ──
  // Called by useVisibilityTracking when a message DOM element enters the viewport.
  // Two responsibilities:
  //   1. Optimistically advance lastReadSeq in the store (instant badge update).
  //   2. Schedule a debounced server flush (coalesces rapid scroll into one API call).
  const reportVisibleSeq = useCallback(
    (visibleSeq: number) => {
      if (!currentUser || !targetKey || !conversationId || visibleSeq <= 0)
        return;
      if (loading || document.visibilityState !== "visible") return;

      // Keep the highest seq seen so far; ignore stale or duplicate reports.
      const nextSeq = Math.max(visibleSeq, pendingReadSeqRef.current ?? 0);
      if (nextSeq <= lastReadSeqRef.current) return;
      pendingReadSeqRef.current = nextSeq;

      // 1. Optimistically advance lastReadSeq so unread count drops immediately.
      if (threadParentId) {
        advanceThreadLastReadSeq(conversationId, threadParentId, nextSeq);
      } else {
        advanceConversationLastReadSeq(conversationId, nextSeq);
      }

      // 2. Debounce: if a flush is already scheduled, it will pick up the new seq.
      if (readCursorTimerRef.current != null) return;
      readCursorTimerRef.current = window.setTimeout(flushReadCursor, 150);
    },
    [
      conversationId,
      loading,
      threadParentId,
      targetKey,
      currentUser,
      flushReadCursor,
      advanceConversationLastReadSeq,
      advanceThreadLastReadSeq,
    ],
  );

  // ── Visibility tracking ──
  const { scheduleBatchVisibilityCheck, resetHighestVisibleSeq } =
    useVisibilityTracking(reportVisibleSeq);

  const buildVisibilityItems = useCallback(() => {
    return messages.map((msg) => ({
      seq: msg.seq,
      element: messageRowRefs.current.get(msg.id) ?? null,
    }));
  }, [messages]);

  const scheduleReadCheck = useCallback(() => {
    const container =
      scrollMode === "inherit"
        ? (externalScrollContainerRef?.current ?? null)
        : internalScrollContainerRef.current;
    scheduleBatchVisibilityCheck(buildVisibilityItems(), container);
  }, [
    scrollMode,
    externalScrollContainerRef,
    buildVisibilityItems,
    scheduleBatchVisibilityCheck,
  ]);

  // ── Derived state ──
  const firstUnreadIndex = messages.findIndex((m) => m.seq > lastReadSeq);
  const unreadCount =
    firstUnreadIndex >= 0 ? messages.length - firstUnreadIndex : 0;
  const hasUnread = unreadCount > 0;

  // ── Scroll management ──
  useEffect(() => {
    const container =
      scrollMode === "inherit"
        ? (externalScrollContainerRef?.current ?? null)
        : internalScrollContainerRef.current;
    if (!container) return;

    const onScroll = () => {
      scheduleReadCheck();
    };
    container.addEventListener("scroll", onScroll);
    return () => container.removeEventListener("scroll", onScroll);
  }, [scrollMode, externalScrollContainerRef, targetKey, scheduleReadCheck]);

  useEffect(() => {
    if (!messages.length || loading) return;
    scheduleReadCheck();
  }, [messages.length, loading, scheduleReadCheck]);

  useEffect(() => {
    const container =
      scrollMode === "inherit"
        ? (externalScrollContainerRef?.current ?? null)
        : internalScrollContainerRef.current;
    if (!container || !messages.length || loading) return;

    const lastMessage = messages[messages.length - 1];
    const isSelfMessage = lastMessage.senderName === currentUser;

    if (isSelfMessage || isNearBottom(container, 100)) {
      const totalMessages = messages.length;
      const approxFirstVisibleIndex = Math.min(
        totalMessages - 1,
        Math.floor(
          (container.scrollTop / container.scrollHeight) * totalMessages,
        ),
      );
      const minVisibleSeq = messages[approxFirstVisibleIndex].seq;
      const scrollDistance = lastMessage.seq - minVisibleSeq;
      const behavior = scrollDistance > 40 ? "instant" : "smooth";
      bottomRef.current?.scrollIntoView({ behavior });
    }
  }, [messages.length, loading]);

  useEffect(() => {
    if (!messages.length || loading) return;
    if (lastTargetRef.current === targetKey) return;
    lastTargetRef.current = targetKey;

    requestAnimationFrame(() => {
      if (firstUnreadIndex >= 0 && firstUnreadAnchorRef.current) {
        firstUnreadAnchorRef.current.scrollIntoView({ block: "start" });
      } else {
        bottomRef.current?.scrollIntoView();
      }
      // Check visibility after initial scroll so already-visible messages are marked read.
      scheduleReadCheck();
    });
  }, [
    targetKey,
    messages.length,
    loading,
    firstUnreadIndex,
    scheduleReadCheck,
  ]);

  // Reset the visibility watermark when the target changes.
  useEffect(() => {
    resetHighestVisibleSeq();
  }, [targetKey, resetHighestVisibleSeq]);

  // ── Handlers ──
  const handleScrollToBottom = useCallback(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, []);

  return (
    <div
      className={`message-list${scrollMode === "inherit" ? " message-list--inherit" : ""}`}
      ref={scrollMode === "inherit" ? undefined : internalScrollContainerRef}
    >
      {loading && messages.length === 0 && (
        <div className="message-list-empty">Loading messages...</div>
      )}
      {!loading && messages.length === 0 && (
        <div className="message-list-empty">{emptyLabel}</div>
      )}
      {messages.map((msg, i) => (
        <div
          key={msg.id}
          ref={(el) => {
            if (el) messageRowRefs.current.set(msg.id, el);
            else messageRowRefs.current.delete(msg.id);
          }}
        >
          <div
            ref={i === firstUnreadIndex ? firstUnreadAnchorRef : undefined}
          />
          {hasUnread && i === firstUnreadIndex && <NewMessageDivider />}
          <MessageItem
            message={msg}
            currentUser={currentUser}
            prevMessage={messages[i - 1]}
            onReply={onReply}
          />
        </div>
      ))}
      <div ref={bottomRef} />
      {hasUnread && (
        <NewMessageBadge
          unreadCount={unreadCount}
          onScrollToBottom={handleScrollToBottom}
        />
      )}
    </div>
  );
}
