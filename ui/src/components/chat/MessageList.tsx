import { useEffect, useRef, useCallback } from "react";
import { useStore } from "../../store";
import { MessageItem } from "./MessageItem";
import { NewMessageDivider } from "./NewMessageDivider";
import { NewMessageBadge } from "./NewMessageBadge";
import type { HistoryMessage } from "./types";
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
  // Messages rendered in visual order.
  messages: HistoryMessage[];
  // True while history is still loading.
  loading: boolean;
  // Highest sequence number the viewer has already read.
  lastReadSeq: number;
  // Logged-in username for self styling.
  currentUser: string | null;
  // Unread message ids tracked for the active target.
  unreadIds: Set<string>;
  // Chooses whether this component owns scrolling or inherits a parent scroller.
  scrollMode?: "internal" | "inherit";
  // Parent scroll container used when scrollMode is inherit.
  externalScrollContainerRef?: RefObject<HTMLDivElement | null>;
  // Optional reply action for each message row.
  onReply?: (message: HistoryMessage) => void;
  // Empty state copy shown when no messages exist.
  emptyLabel?: string;
}

function SeenTracker({
  messageId,
  messageContent,
  targetKey,
}: {
  messageId: string;
  messageContent?: string;
  targetKey: string;
}) {
  const markUnreadAsSeen = useStore((s) => s.markUnreadAsSeen);
  useEffect(() => {
    markUnreadAsSeen(targetKey, messageId, messageContent);
  }, [targetKey, messageId, messageContent, markUnreadAsSeen]);
  return null;
}

export function MessageList({
  targetKey,
  messages,
  loading,
  lastReadSeq,
  currentUser,
  unreadIds,
  scrollMode = "internal",
  externalScrollContainerRef,
  onReply,
  emptyLabel = "No messages yet. Be the first to say something!",
}: MessageListProps) {
  const bottomRef = useRef<HTMLDivElement>(null);
  const internalScrollContainerRef = useRef<HTMLDivElement>(null);
  const firstUnreadAnchorRef = useRef<HTMLDivElement>(null);
  const lastTargetRef = useRef<string>("");
  const clearAllUnread = useStore((s) => s.clearAllUnread);

  const firstUnreadIndex = messages.findIndex((m) => m.seq > lastReadSeq);
  const hasUnread = unreadIds.size > 0;

  const handleScrollToBottom = useCallback(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, []);

  useEffect(() => {
    const container =
      scrollMode === "inherit"
        ? (externalScrollContainerRef?.current ?? null)
        : internalScrollContainerRef.current;
    if (!container) return;

    let isAtBottom = isNearBottom(container);

    const onScroll = () => {
      const transition = getBottomTransition(isAtBottom, container);
      const nowAtBottom = isNearBottom(container);
      if (transition === "entered") {
        console.log("scrolled to bottom, clearAllUnread:", targetKey);
        clearAllUnread(targetKey);
      } else if (transition === "left") {
        console.log("scrolled from bottom");
      }
      isAtBottom = nowAtBottom;
    };
    container.addEventListener("scroll", onScroll);
    return () => container.removeEventListener("scroll", onScroll);
  }, [scrollMode, externalScrollContainerRef, targetKey, clearAllUnread]);

  useEffect(() => {
    const container =
      scrollMode === "inherit"
        ? (externalScrollContainerRef?.current ?? null)
        : internalScrollContainerRef.current;
    if (!container || !messages.length || loading) return;

    if (isNearBottom(container, 100)) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
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
    });
  }, [targetKey, messages.length, loading, firstUnreadIndex]);

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
      {hasUnread && <NewMessageDivider />}
      {messages.map((msg, i) => (
        <div key={msg.id}>
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
          {unreadIds.has(msg.id) && (
            <SeenTracker
              messageId={msg.id}
              messageContent={msg.content}
              targetKey={targetKey}
            />
          )}
        </div>
      ))}
      <div ref={bottomRef} />
      {hasUnread && (
        <NewMessageBadge
          unreadCount={unreadIds.size}
          onScrollToBottom={handleScrollToBottom}
        />
      )}
    </div>
  );
}
