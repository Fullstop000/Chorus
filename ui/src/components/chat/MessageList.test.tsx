import { renderToStaticMarkup } from "react-dom/server";
import { describe, it, expect, vi } from "vitest";
import { MessageList } from "./MessageList";
import type { HistoryMessage } from "../../data/chat";

// MessageList calls the store in TWO shapes:
//   useStore() — returns full state (destructured for advanceConversationLastReadSeq)
//   useStore((s) => s.setCurrentTaskDetail) — selector form for the task_card branch
// The mock must support both or the test crashes before the assertion.
vi.mock("../../store", () => {
  const state = {
    advanceConversationLastReadSeq: () => {},
    setCurrentTaskDetail: () => {},
  };
  return {
    useStore: (selector?: (s: typeof state) => unknown) =>
      selector ? selector(state) : state,
  };
});
// Partial-mock, not a full stub: `data/channels.ts` uses `queryOptions` at
// module-load time, so replacing the whole react-query module with a stub
// crashes the import graph before the test runs. Spread the real exports and
// only override `useQueryClient`, which is what MessageList actually calls.
vi.mock("@tanstack/react-query", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@tanstack/react-query")>();
  return {
    ...actual,
    useQueryClient: () => ({
      setQueryData: () => {},
      invalidateQueries: () => {},
    }),
  };
});

describe("MessageList — system message routing", () => {
  it("renders one TaskEventRow per task_event system message", () => {
    const msgs: HistoryMessage[] = [
      {
        id: "m1",
        seq: 1,
        content: JSON.stringify({
          kind: "task_event",
          action: "claimed",
          taskNumber: 7,
          title: "wire up",
          subChannelId: "s",
          actor: "alice",
          prevStatus: "todo",
          nextStatus: "in_progress",
          claimedBy: "alice",
        }),
        senderName: "system",
        senderType: "system",
        createdAt: "2026-04-23T10:00:00Z",
        senderDeleted: false,
      },
      {
        id: "m2",
        seq: 2,
        content: JSON.stringify({
          kind: "task_event",
          action: "status_changed",
          taskNumber: 7,
          title: "wire up",
          subChannelId: "s",
          actor: "alice",
          prevStatus: "in_progress",
          nextStatus: "in_review",
        }),
        senderName: "system",
        senderType: "system",
        createdAt: "2026-04-23T10:05:00Z",
        senderDeleted: false,
      },
    ];
    const html = renderToStaticMarkup(
      <MessageList
        targetKey="eng"
        conversationId="cid"
        messages={msgs}
        loading={false}
        lastReadSeq={0}
        currentUser="alice"
      />,
    );
    // Each task_event row renders independently — no per-task suppression.
    const claimed = (html.match(/data-action="claimed"/g) ?? []).length;
    const statusChanged = (html.match(/data-action="status_changed"/g) ?? [])
      .length;
    expect(claimed).toBe(1);
    expect(statusChanged).toBe(1);
  });

  it("does not render a TaskCard when the task row is unknown to the store", () => {
    // Without a populated tasks store, TaskCardContainer returns null. The
    // wrapper div still anchors visibility tracking, but no card body.
    const msgs: HistoryMessage[] = [
      {
        id: "m1",
        seq: 1,
        content: JSON.stringify({
          kind: "task_card",
          taskId: "unknown-id",
          taskNumber: 7,
          title: "wire up",
          status: "todo",
          owner: null,
          createdBy: "alice",
        }),
        senderName: "system",
        senderType: "system",
        createdAt: "2026-04-23T10:00:00Z",
        senderDeleted: false,
      },
    ];
    const html = renderToStaticMarkup(
      <MessageList
        targetKey="eng"
        conversationId="cid"
        messages={msgs}
        loading={false}
        lastReadSeq={0}
        currentUser="alice"
      />,
    );
    expect(html).not.toContain('data-testid="task-card-7"');
  });
});
