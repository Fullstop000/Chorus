import { renderToStaticMarkup } from "react-dom/server";
import { describe, it, expect, vi } from "vitest";
import { MessageList } from "./MessageList";
import type { HistoryMessage } from "../../data/chat";

// MessageList calls the store in TWO shapes:
//   useStore() — returns full state (destructured for advanceConversationLastReadSeq)
//   useStore((s) => s.setCurrentTaskDetail) — selector form for our new branch
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

describe("MessageList — task_event rendering", () => {
  it("renders exactly one task-thread for a sequence of events on the same task", () => {
    const msgs: HistoryMessage[] = [
      {
        id: "m1",
        seq: 1,
        content: JSON.stringify({
          kind: "task_event",
          action: "created",
          taskNumber: 7,
          title: "wire up",
          subChannelId: "s",
          actor: "alice",
          nextStatus: "todo",
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
    const occurrences = (html.match(/data-testid="task-thread-7"/g) ?? [])
      .length;
    expect(occurrences).toBe(1);
    expect(html).toContain('data-state="in_progress"');
  });
});
