import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ChannelInfo } from "../../data";
import type { useHistory } from "../../hooks/useHistory";

/**
 * Minimal `useHistory` return stub. MessageInput only calls
 * `history.appendMessage` during `handleSend`, which the static-render tests
 * never trigger, so a no-op stub is enough to satisfy the type.
 */
function stubHistory(): ReturnType<typeof useHistory> {
  return {
    messages: [],
    loading: false,
    error: null,
    lastReadSeq: 0,
    loadedTarget: null,
    refresh: (() => Promise.resolve({} as never)) as ReturnType<
      typeof useHistory
    >["refresh"],
    appendMessage: () => {},
  };
}

function renderWithProviders(ui: React.ReactElement): string {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return renderToStaticMarkup(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
}

const parentChannel: ChannelInfo = {
  id: "11111111-1111-1111-1111-111111111111",
  name: "eng",
  joined: true,
  channel_type: "team",
};

/**
 * Zustand's `useSyncExternalStore`-based subscription does not surface the
 * current store snapshot during `react-dom/server.renderToStaticMarkup`,
 * so we stub the store module directly for these render tests. The real
 * store is exercised by TaskDetail.test.tsx and integration tests.
 */
vi.mock("../../store", () => {
  const state = {
    currentUser: "alice",
    currentUserId: "alice",
    currentChannel: parentChannel as ChannelInfo | null,
    pushToast: () => {},
  };
  const useStore = ((selector?: (s: typeof state) => unknown) =>
    selector ? selector(state) : state) as unknown as {
    (): typeof state;
    <T>(selector: (s: typeof state) => T): T;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    __set: (patch: Partial<typeof state>) => void;
  };
  useStore.__set = (patch: Partial<typeof state>) => {
    Object.assign(state, patch);
  };
  return { useStore };
});

// Hooks that internally use react-query against the real backend — stub them
// to avoid needing a fetch mock.
vi.mock("../../hooks/data", () => ({
  useAgents: () => [],
  useTeams: () => [],
  useHumans: () => [],
  useChannels: () => ({
    allChannels: [],
    channels: [],
    systemChannels: [],
    dmChannels: [],
  }),
  useChannelMembers: () => [],
}));

// MessageInput now reads currentChannel from the URL via useCurrentChannel.
// Stub the resolver so the test doesn't need a real Router + query cache.
let mockedChannel: ChannelInfo | null = parentChannel;
vi.mock("../../hooks/useRouteSubject", () => ({
  useCurrentChannel: () => mockedChannel,
}));

// Import AFTER mocks so MessageInput picks them up.
const { MessageInput } = await import("./MessageInput");

beforeEach(() => {
  mockedChannel = parentChannel;
});

afterEach(() => {
  mockedChannel = parentChannel;
});

describe("MessageInput create-task checkbox", () => {
  it("renders the 'also create as task' checkbox by default when a channel is selected", () => {
    const html = renderWithProviders(
      <MessageInput
        target="eng"
        conversationId={parentChannel.id ?? null}
        history={stubHistory()}
      />,
    );

    expect(html).toMatch(/also create as a task/i);
  });

  it("hides the 'also create as task' checkbox when hideCreateTaskCheckbox is set", () => {
    // Leaves currentChannel pointing at the parent — mirroring the
    // TaskDetail case where the user is inside a sub-channel while the
    // store still references the parent channel.
    const html = renderWithProviders(
      <MessageInput
        target="eng__task-1"
        conversationId="22222222-2222-2222-2222-222222222222"
        history={stubHistory()}
        hideCreateTaskCheckbox
      />,
    );

    expect(html).not.toMatch(/also create as a task/i);
  });
});
