import { useEffect, useRef, useCallback } from "react";
import { useStore } from "../../store";
import { MessageItem } from "./MessageItem";
import { NewMessageDivider } from "./NewMessageDivider";
import { NewMessageBadge } from "./NewMessageBadge";
import type { HistoryMessage } from "./types";
import "./MessageList.css";

interface MessageListProps {
  targetKey: string;
  messages: HistoryMessage[];
  loading: boolean;
  lastReadSeq: number;
  currentUser: string | null;
  unreadIds: Set<string>;
  onReply?: (message: HistoryMessage) => void;
  onRetry?: (message: HistoryMessage) => void;
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
  onReply,
  onRetry,
  emptyLabel = "No messages yet. Be the first to say something!",
}: MessageListProps) {
  const bottomRef = useRef<HTMLDivElement>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const firstUnreadAnchorRef = useRef<HTMLDivElement>(null);
  const lastTargetRef = useRef<string>("");
  const clearAllUnread = useStore((s) => s.clearAllUnread);

  const firstUnreadIndex = messages.findIndex((m) => m.seq > lastReadSeq);

  const handleScrollToBottom = useCallback(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    clearAllUnread(targetKey);
  }, [targetKey, clearAllUnread]);

  useEffect(() => {
    const container = scrollContainerRef.current;
    if (!container || !messages.length || loading) return;

    const distFromBottom =
      container.scrollHeight - container.scrollTop - container.clientHeight;
    if (distFromBottom < 100) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
      clearAllUnread(targetKey);
    }
  }, [messages.length, loading, targetKey, clearAllUnread]);

  useEffect(() => {
    if (!messages.length || loading) return;
    if (lastTargetRef.current === targetKey) return;
    lastTargetRef.current = targetKey;

    requestAnimationFrame(() => {
      if (firstUnreadIndex >= 0 && firstUnreadAnchorRef.current) {
        firstUnreadAnchorRef.current.scrollIntoView({ block: "start" });
      } else {
        bottomRef.current?.scrollIntoView();
        clearAllUnread(targetKey);
      }
    });
  }, [targetKey, messages.length, loading, firstUnreadIndex, clearAllUnread]);

  const hasUnread = unreadIds.size > 0;

  return (
    <div className="message-list" ref={scrollContainerRef}>
      {loading && messages.length === 0 && (
        <div className="message-list-empty">Loading messages...</div>
      )}
      {!loading && messages.length === 0 && (
        <div className="message-list-empty">{emptyLabel}</div>
      )}
      {hasUnread && firstUnreadIndex === 0 && <NewMessageDivider />}
      {messages.map((msg, i) => (
        <div key={msg.id}>
          <div
            ref={i === firstUnreadIndex ? firstUnreadAnchorRef : undefined}
          />
          {i === firstUnreadIndex && firstUnreadIndex > 0 && (
            <NewMessageDivider />
          )}
          <MessageItem
            message={msg}
            currentUser={currentUser}
            prevMessage={messages[i - 1]}
            onReply={onReply}
            onRetry={onRetry}
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
