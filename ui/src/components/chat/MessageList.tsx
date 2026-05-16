import { useEffect, useRef, useCallback, useMemo } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { MessageItem } from "./MessageItem";
import { Telescope } from "./Telescope";
import { NewMessageDivider } from "./NewMessageDivider";
import { NewMessageBadge } from "./NewMessageBadge";
import { useVisibilityTracking } from "../../hooks/useVisibilityTracking";
import { updateReadCursor, historyQueryKeys } from "../../data";
import type { HistoryMessage, HistoryResponse } from "../../data";
import { useStore } from "../../store";
import { useTraceStore } from "../../store/traceStore";
import { useTaskEventLog } from "../../hooks/useTaskEventLog";
import { TaskEventMessage } from "./TaskEventMessage";
import { taskDetailPath } from "../../lib/routes";
import "./MessageList.css";
import type { RefObject } from "react";
import type { AgentTrace } from "../../store/traceStore";

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

export function getScopedAgentNames(
  targetKey: string,
  messages: Pick<HistoryMessage, "senderName" | "senderType">[],
  conversationAgentNames: string[] = [],
): Set<string> {
  const names = new Set(conversationAgentNames);
  if (targetKey.startsWith("dm:@")) {
    names.add(targetKey.slice("dm:@".length));
  }
  for (const msg of messages) {
    if (msg.senderType === "agent") names.add(msg.senderName);
  }
  return names;
}

export function traceBelongsToConversation(
  conversationId: string | null,
  agentName: string,
  trace: Pick<AgentTrace, "channelId">,
  scopedAgentNames: ReadonlySet<string>,
): boolean {
  if (conversationId && trace.channelId) {
    return trace.channelId === conversationId;
  }
  return scopedAgentNames.has(agentName);
}

interface MessageListProps {
  // Stable store key for the current channel or DM.
  targetKey: string;
  // Conversation id for read-cursor tracking.
  conversationId: string | null;
  // Agent names that belong to the current conversation. Used as a fallback
  // for trace frames that predate channelId propagation.
  conversationAgentNames?: string[];
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
  // Empty state copy shown when no messages exist.
  emptyLabel?: string;
}

export function MessageList({
  targetKey,
  conversationId,
  conversationAgentNames = [],
  messages,
  loading,
  lastReadSeq,
  currentUser,
  scrollMode = "internal",
  externalScrollContainerRef,
  emptyLabel = "No messages yet. Be the first to say something!",
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
  const queryKey = historyQueryKeys.history(conversationId ?? "");
  const { advanceConversationLastReadSeq } = useStore();
  const taskEventIndex = useTaskEventLog(messages);
  const navigate = useNavigate();
  const location = useLocation();

  // ── Sync refs with props ──
  useEffect(() => {
    lastReadSeqRef.current = lastReadSeq;
  }, [lastReadSeq]);

  useEffect(() => {
    pendingReadSeqRef.current = null;
    lastReadSeqRef.current = lastReadSeq;
    // Cancel any pending flush left over from the previous target. Its
    // callback closes over the old `targetKey`/`conversationId` and would
    // bail at the `activeTargetRef.current !== targetKey` guard, leaving
    // `readCursorTimerRef` non-null and blocking the new target's flush
    // from ever being scheduled.
    if (readCursorTimerRef.current != null) {
      window.clearTimeout(readCursorTimerRef.current);
      readCursorTimerRef.current = null;
    }
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
      await updateReadCursor(conversationId!, flushSeq);
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
  }, [conversationId, targetKey, queryClient, queryKey]);

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
      advanceConversationLastReadSeq(conversationId, nextSeq);

      // 2. Debounce: if a flush is already scheduled, it will pick up the new seq.
      if (readCursorTimerRef.current != null) return;
      readCursorTimerRef.current = window.setTimeout(flushReadCursor, 150);
    },
    [
      conversationId,
      loading,
      targetKey,
      currentUser,
      flushReadCursor,
      advanceConversationLastReadSeq,
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

  // ── Telescope: determine which messages show an agent trace ──
  const traces = useTraceStore((s) => s.traces);
  const expandedAgents = useTraceStore((s) => s.expandedAgents);
  const toggleExpanded = useTraceStore((s) => s.toggleExpanded);

  const scopedAgentNames = useMemo(
    () => getScopedAgentNames(targetKey, messages, conversationAgentNames),
    [conversationAgentNames, messages, targetKey],
  );

  // Translate trace.agent_id → display name for the scopedAgentNames
  // fallback path inside traceBelongsToConversation. Built from agent
  // messages in this conversation; agents that haven't spoken here yet
  // resolve to an empty string and fail the scope check (correct: no
  // signal that they belong to this conversation).
  const agentNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const msg of messages) {
      if (msg.senderType === "agent") {
        map.set(msg.senderId, msg.senderName);
      }
    }
    return map;
  }, [messages]);

  const traceInCurrentConversation = useCallback(
    (agentId: string, trace: (typeof traces)[string]) => {
      return traceBelongsToConversation(
        conversationId,
        agentNameById.get(agentId) ?? "",
        trace,
        scopedAgentNames,
      );
    },
    [conversationId, scopedAgentNames, agentNameById],
  );

  // Map: agentId:runId → message id with that runId (for exact binding)
  // Map: agentId → last message id (fallback for inactive traces with no runId match)
  // Map: runId → first message id (only first message per run shows static telescope)
  const { agentRunIdMsgId, agentLastMsgId, firstMsgIdPerRun } = useMemo(() => {
    const agentRunIdMsgId = new Map<string, string>();
    const agentLastMsgId = new Map<string, string>();
    const firstMsgIdPerRun = new Map<string, string>();
    for (const msg of messages) {
      if (msg.senderType === "agent") {
        agentLastMsgId.set(msg.senderId, msg.id);
        if (msg.runId) {
          agentRunIdMsgId.set(`${msg.senderId}:${msg.runId}`, msg.id);
          if (!firstMsgIdPerRun.has(msg.runId)) {
            firstMsgIdPerRun.set(msg.runId, msg.id);
          }
        }
      }
    }
    return { agentRunIdMsgId, agentLastMsgId, firstMsgIdPerRun };
  }, [messages]);

  // Collect active run IDs from live traces
  const activeRunIds = new Set<string>();
  for (const [agentId, trace] of Object.entries(traces)) {
    if (
      trace.isActive &&
      trace.runId &&
      traceInCurrentConversation(agentId, trace)
    ) {
      activeRunIds.add(trace.runId);
    }
  }

  // Compute orphaned traces: active traces whose runId has no matching message.
  // These are rendered at the bottom as "agent working" indicators.
  const orphanedTraces: Array<[string, (typeof traces)[string]]> = [];
  for (const [agentId, trace] of Object.entries(traces)) {
    if (!trace.isActive) continue;
    if (!traceInCurrentConversation(agentId, trace)) continue;
    const matchKey = `${agentId}:${trace.runId}`;
    if (!agentRunIdMsgId.has(matchKey)) {
      orphanedTraces.push([agentId, trace]);
    }
  }

  // Wrapper exists so NewMessageBadge can be `position: absolute` against a
  // stable non-scrolling parent. Before this split the badge used
  // `position: fixed` and floated at the viewport bottom-right, which looked
  // correct inside the main ChatPanel (it occupies most of the viewport) but
  // broke inside TaskDetail where the chat is a sub-region and the badge
  // ended up hovering over the composer.
  return (
    <div className={`message-list-wrapper${scrollMode === "inherit" ? " message-list-wrapper--inherit" : ""}`}>
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
      {messages.map((msg, i) => {
        // Task-event system messages short-circuit to the TaskEventMessage
        // renderer. Suppress repeated events for the same task so we render
        // one card per task, anchored at its latest event. The hook already
        // parsed the payload once when building `taskEventIndex`; the render
        // loop just asks "does this seq belong to a task?" — O(1), no JSON
        // re-parse.
        const taskNumber = taskEventIndex.taskNumberBySeq.get(msg.seq);
        if (msg.senderType === "system" && taskNumber !== undefined) {
          const state = taskEventIndex.byTaskNumber.get(taskNumber);
          const isLatestForTask = state && state.latestSeq === msg.seq;

          // Even when we suppress the task-card render (not the latest
          // event), we still need the wrapper div so visibility tracking
          // hits the row AND the unread divider anchors on the right seq.
          // Returning null here loses the unread anchor entirely.
          return (
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
              {isLatestForTask && state && (
                <TaskEventMessage
                  taskState={state}
                  onOpen={() => {
                    // Strip the `#` prefix from targetKey to get the channel slug
                    // for the URL. DM-scoped tasks (where targetKey starts with
                    // `dm:@`) keep their odd-looking slug; encodeURIComponent makes
                    // the URL valid even though it's not shareable.
                    const slug = targetKey?.startsWith("#")
                      ? targetKey.slice(1)
                      : targetKey;
                    if (!slug) return;
                    navigate(taskDetailPath(slug, taskNumber), {
                      state: { from: location.pathname },
                    });
                  }}
                />
              )}
            </div>
          );
        }

        // Bind trace to message:
        // 1. Exact runId match on the LAST message for this run → telescope tracks latest message
        // 2. Inactive trace with no runId match → fallback to last message by agent
        // 3. Active trace with no match → shown as orphaned at bottom, not here
        let agentTrace: (typeof traces)[string] | undefined;
        if (msg.senderType === "agent") {
          const trace = traces[msg.senderId];
          if (trace && traceInCurrentConversation(msg.senderId, trace)) {
            const matchKey = `${msg.senderId}:${trace.runId}`;
            if (
              msg.runId &&
              msg.runId === trace.runId &&
              agentRunIdMsgId.get(matchKey) === msg.id
            ) {
              agentTrace = trace;
            } else if (
              !trace.isActive &&
              !agentRunIdMsgId.has(matchKey) &&
              agentLastMsgId.get(msg.senderId) === msg.id
            ) {
              agentTrace = trace;
            }
          }
        }
        return (
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
              traceData={agentTrace}
              showTraceSummary={
                !msg.runId || firstMsgIdPerRun.get(msg.runId) === msg.id
              }
              isRunActive={
                !!msg.runId && activeRunIds.has(msg.runId)
              }
              isTraceExpanded={expandedAgents[msg.senderId] ?? true}
              onToggleTrace={() => toggleExpanded(msg.senderId)}
            />
          </div>
        );
      })}
      {orphanedTraces.map(([agentId, trace]) => (
        <div key={`pending-${agentId}`} className="pending-trace-wrapper">
          <span className="pending-trace-agent">{agentNameById.get(agentId) ?? agentId}</span>
          <Telescope
            agentName={agentNameById.get(agentId) ?? agentId}
            runId={trace.runId}
            events={trace.events as never[]}
            isActive={trace.isActive}
            isError={trace.isError}
            isExpanded={expandedAgents[agentId] ?? true}
            onToggleExpand={() => toggleExpanded(agentId)}
          />
        </div>
      ))}
      <div ref={bottomRef} />
    </div>
      {hasUnread && (
        <NewMessageBadge
          unreadCount={unreadCount}
          onScrollToBottom={handleScrollToBottom}
        />
      )}
    </div>
  );
}
