import { renderToStaticMarkup } from "react-dom/server";
import { describe, it, expect, vi } from "vitest";
import { MemoryRouter } from "react-router-dom";
import {
  getScopedAgentNames,
  MessageList,
  traceBelongsToConversation,
} from "./MessageList";
import type { HistoryMessage, MessagePayload } from "../../data/chat";

// MessageList calls `useStore()` (destructured) and `useStore(selector)`.
// The mock supports both shapes.
vi.mock("../../store", () => {
  const state = {
    advanceConversationLastReadSeq: () => {},
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
vi.mock("../../hooks/data", () => ({
  useAgents: () => [],
}));

describe("MessageList — task_event rendering", () => {
  it("renders exactly one task-thread for a sequence of events on the same task", () => {
    const msgs: HistoryMessage[] = [
      {
        id: "m1",
        seq: 1,
        content: 'alice created #7 "wire up"',
        senderName: "system",
        senderType: "system",
        createdAt: "2026-04-23T10:00:00Z",
        senderDeleted: false,
        payload: {
          kind: "task_event",
          action: "created",
          taskNumber: 7,
          title: "wire up",
          subChannelId: "s",
          actor: "alice",
          nextStatus: "todo",
        } as MessagePayload,
      },
      {
        id: "m2",
        seq: 2,
        content: 'alice claimed #7 "wire up" (now in_progress)',
        senderName: "system",
        senderType: "system",
        createdAt: "2026-04-23T10:05:00Z",
        senderDeleted: false,
        payload: {
          kind: "task_event",
          action: "claimed",
          taskNumber: 7,
          title: "wire up",
          subChannelId: "s",
          actor: "alice",
          prevStatus: "todo",
          nextStatus: "in_progress",
          claimedBy: "alice",
        } as MessagePayload,
      },
    ];
    const html = renderToStaticMarkup(
      <MemoryRouter>
        <MessageList
          targetKey="eng"
          conversationId="cid"
          messages={msgs}
          loading={false}
          lastReadSeq={0}
          currentUser="alice"
        />
      </MemoryRouter>,
    );
    const occurrences = (html.match(/data-testid="task-thread-7"/g) ?? [])
      .length;
    expect(occurrences).toBe(1);
    expect(html).toContain('data-state="in_progress"');
  });
});

describe("MessageList — live Telescope scoping", () => {
  it("rejects another agent's active trace in the current DM", () => {
    const scoped = getScopedAgentNames(
      "dm:@opencode-1800",
      [],
      ["opencode-1800"],
    );

    expect(
      traceBelongsToConversation(
        "dm-opencode",
        "gemini-e192",
        { channelId: "dm-gemini" },
        scoped,
      ),
    ).toBe(false);
  });

  it("accepts an active trace when it belongs to the current conversation", () => {
    const scoped = getScopedAgentNames(
      "dm:@opencode-1800",
      [],
      ["opencode-1800"],
    );

    expect(
      traceBelongsToConversation(
        "dm-opencode",
        "opencode-1800",
        { channelId: "dm-opencode" },
        scoped,
      ),
    ).toBe(true);
  });

  it("falls back to current conversation agents when old trace frames have no channel id", () => {
    const scoped = getScopedAgentNames(
      "dm:@opencode-1800",
      [],
      ["opencode-1800"],
    );

    expect(
      traceBelongsToConversation(
        "dm-opencode",
        "opencode-1800",
        { channelId: undefined },
        scoped,
      ),
    ).toBe(true);
    expect(
      traceBelongsToConversation(
        "dm-opencode",
        "gemini-e192",
        { channelId: undefined },
        scoped,
      ),
    ).toBe(false);
  });
});
