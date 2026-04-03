import { useMemo, useRef, useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import { useStore } from "./store/uiStore";
import {
  whoamiQuery,
  agentsQuery,
  channelsQuery,
  teamsQuery,
  humansQuery,
  inboxQuery,
  ensureDirectMessageConversation,
  channelQueryKeys,
  getConversationInboxNotification,
} from "./data";
import type { AgentInfo, ChannelInfo } from "./data";
import {
  dmConversationNameForParticipants,
  ensureInboxConversations,
  buildConversationRegistry,
  mergeInboxNotificationRefresh,
  type InboxState,
} from "./inbox";
import { isVisibleSidebarChannel } from "./pages/Sidebar/sidebarChannels";
import { getRealtimeSession } from "./transport/realtimeSession";
import { queryClient as appQueryClient } from "./lib/queryClient";
import type { ReadCursorAckPayload } from "./inbox";
import { MainPanel } from "./pages/MainPanel";
import { Sidebar } from "./pages/Sidebar";

function loadAppData(
  currentUser: string,
  shellBootstrapped: boolean,
  channelsData?: import("./data").ChannelInfo[],
) {
  const whoamiResult = useQuery(whoamiQuery);
  const agentsResult = useQuery(agentsQuery(currentUser));
  const channelsResult = useQuery(channelsQuery(currentUser));
  const teamsResult = useQuery(teamsQuery(currentUser));
  const humansResult = useQuery(humansQuery(currentUser));
  const inboxResult = useQuery(
    inboxQuery(currentUser, shellBootstrapped, channelsData),
  );

  return {
    whoamiQuery: whoamiResult,
    agentsQuery: agentsResult,
    channelsQuery: channelsResult,
    teamsQuery: teamsResult,
    humansQuery: humansResult,
    inboxQuery: inboxResult,
  };
}

function syncWhoami(
  username: string | undefined,
  currentUser: string,
  setCurrentUser: (u: string) => void,
  resetUserSession: () => void,
): void {
  useEffect(() => {
    if (!username) return;
    if (username === currentUser) return;
    if (currentUser) resetUserSession();
    setCurrentUser(username);
  }, [username, currentUser, setCurrentUser, resetUserSession]);
}

function mirrorChannels(
  allChannels: ChannelInfo[],
  updateInboxState: (u: (c: InboxState) => InboxState) => void,
): void {
  useEffect(() => {
    if (!allChannels.length) return;
    updateInboxState((current) =>
      ensureInboxConversations(current, allChannels),
    );
  }, [allChannels, updateInboxState]);
}

function autoSelectChannel(params: {
  shellBootstrapped: boolean;
  channels: ChannelInfo[];
  systemChannels: ChannelInfo[];
  setCurrentChannel: (channel: ChannelInfo | null) => void;
}): void {
  const { shellBootstrapped, channels, systemChannels, setCurrentChannel } =
    params;

  useEffect(() => {
    const { currentAgent, currentChannel } = useStore.getState();
    if (currentAgent) return;
    if (!channels.length && !systemChannels.length) return;

    const joinedChannels = [
      ...systemChannels.filter((c) => c.joined),
      ...channels.filter(isVisibleSidebarChannel),
    ];

    if (
      currentChannel &&
      joinedChannels.some(
        (c) => c.id === currentChannel.id || c.name === currentChannel.name,
      )
    )
      return;

    setCurrentChannel(joinedChannels[0] ?? null);
  }, [shellBootstrapped, channels, systemChannels, setCurrentChannel]);
}

function ensureAgentDm(params: {
  currentUser: string;
  currentAgentName: string | null;
  dmChannels: ChannelInfo[];
  queryClient: QueryClient;
  updateInboxState: (u: (c: InboxState) => InboxState) => void;
}): void {
  const {
    currentUser,
    currentAgentName,
    dmChannels,
    queryClient,
    updateInboxState,
  } = params;

  useEffect(() => {
    if (!currentUser || !currentAgentName) return;
    const dmName = dmConversationNameForParticipants(
      currentUser,
      currentAgentName,
    );
    if (dmChannels.some((ch: ChannelInfo) => ch.name === dmName)) return;

    let cancelled = false;
    ensureDirectMessageConversation(currentAgentName)
      .then((channel) => {
        if (cancelled) return;
        queryClient.setQueryData<ChannelInfo[]>(
          channelQueryKeys.channels(currentUser),
          (current = []) => {
            if (
              current.some(
                (ch: ChannelInfo) =>
                  ch.id === channel.id || ch.name === channel.name,
              )
            ) {
              return current;
            }
            return [...current, channel];
          },
        );
        updateInboxState((current: InboxState) =>
          ensureInboxConversations(current, [channel]),
        );
      })
      .catch((error) => {
        if (!cancelled)
          console.error("Failed to ensure DM conversation", error);
      });

    return () => {
      cancelled = true;
    };
  }, [
    currentUser,
    dmChannels,
    currentAgentName,
    queryClient,
    updateInboxState,
  ]);
}

function parseThreadParentId(raw: unknown): string | undefined {
  return typeof raw === "string" && raw.length > 0 ? raw : undefined;
}

function subscribeInbox(params: {
  currentUser: string;
  shellBootstrapped: boolean;
  systemChannels: ChannelInfo[];
  channels: ChannelInfo[];
  dmChannels: ChannelInfo[];
  agents: AgentInfo[];
  updateInboxState: (u: (c: InboxState) => InboxState) => void;
}): void {
  const {
    currentUser,
    shellBootstrapped,
    systemChannels,
    channels,
    dmChannels,
    agents,
    updateInboxState,
  } = params;

  const inboxRefreshInFlight = useRef<Set<string>>(new Set());
  const inboxRefreshPending = useRef<Map<string, [string, string | undefined]>>(
    new Map(),
  );

  useEffect(() => {
    if (!currentUser || !shellBootstrapped) return;

    const conversationRegistry = buildConversationRegistry({
      currentUser,
      systemChannels,
      channels,
      dmChannels,
      agents,
    });
    const targets = conversationRegistry.map(
      (e) => `conversation:${e.conversationId}`,
    );
    if (targets.length === 0) return;

    const scheduleInboxRefresh = (
      key: string,
      channelId: string,
      threadParentId: string | undefined,
    ): void => {
      inboxRefreshInFlight.current.add(key);
      void getConversationInboxNotification(channelId, threadParentId)
        .then((payload) => {
          updateInboxState((current: InboxState) =>
            mergeInboxNotificationRefresh(current, payload),
          );
        })
        .catch((error) => {
          console.error("Failed to refresh inbox after message", error);
        })
        .finally(() => {
          inboxRefreshInFlight.current.delete(key);
          const pending = inboxRefreshPending.current.get(key);
          if (pending) {
            inboxRefreshPending.current.delete(key);
            scheduleInboxRefresh(key, pending[0], pending[1]);
          }
        });
    };

    return getRealtimeSession(currentUser).subscribe({
      targets,
      onFrame: (frame) => {
        if (frame.type === "error") {
          console.error("Inbox realtime subscription failed", frame.message);
          return;
        }
        if (frame.event.eventType === "message.created") {
          const channelId = frame.event.channelId;
          const threadParentId = parseThreadParentId(
            frame.event.payload.threadParentId,
          );
          const key = `${channelId}:${threadParentId ?? ""}`;
          if (inboxRefreshInFlight.current.has(key)) {
            inboxRefreshPending.current.set(key, [channelId, threadParentId]);
          } else {
            void getConversationInboxNotification(channelId, threadParentId)
              .then((payload) => {
                updateInboxState((current: InboxState) =>
                  mergeInboxNotificationRefresh(current, payload),
                );
                inboxRefreshInFlight.current.delete(key);
                const pending = inboxRefreshPending.current.get(key);
                if (pending) {
                  inboxRefreshPending.current.delete(key);
                  void getConversationInboxNotification(
                    pending[0],
                    pending[1] || undefined,
                  ).then((p) =>
                    updateInboxState((c: InboxState) =>
                      mergeInboxNotificationRefresh(c, p),
                    ),
                  );
                }
              })
              .catch((error) => {
                inboxRefreshInFlight.current.delete(key);
                console.error("Failed to refresh inbox after message", error);
              });
            inboxRefreshInFlight.current.add(key);
          }
          return;
        }
      },
    });
  }, [
    agents,
    channels,
    currentUser,
    dmChannels,
    shellBootstrapped,
    systemChannels,
    updateInboxState,
  ]);
}

export default function App() {
  const currentUser = useStore((s) => s.currentUser);
  const shellBootstrapped = useStore((s) => s.shellBootstrapped);
  const setCurrentUser = useStore((s) => s.setCurrentUser);
  const resetUserSession = useStore((s) => s.resetUserSession);
  const setCurrentChannel = useStore((s) => s.setCurrentChannel);
  const setShellBootstrapped = useStore((s) => s.setShellBootstrapped);
  const updateInboxState = useStore((s) => s.updateInboxState);

  const prevAllChannelsRef = useRef<ChannelInfo[] | undefined>(undefined);

  const queries = loadAppData(
    currentUser,
    shellBootstrapped,
    prevAllChannelsRef.current,
  );
  const { whoamiQuery, agentsQuery, channelsQuery, inboxQuery } = queries;

  const channelsData = channelsQuery.data;
  prevAllChannelsRef.current = channelsData?.allChannels;

  const agents = useMemo(() => agentsQuery.data ?? [], [agentsQuery.data]);
  const allChannels = channelsData?.allChannels ?? [];
  const channels = channelsData?.channels ?? [];
  const systemChannels = channelsData?.systemChannels ?? [];
  const dmChannels = channelsData?.dmChannels ?? [];

  syncWhoami(whoamiQuery.data, currentUser, setCurrentUser, resetUserSession);

  mirrorChannels(allChannels, updateInboxState);

  autoSelectChannel({
    shellBootstrapped,
    channels,
    systemChannels,
    setCurrentChannel,
  });

  const currentAgentName = useStore((s) => s.currentAgent?.name ?? null);
  ensureAgentDm({
    currentUser,
    currentAgentName,
    dmChannels,
    queryClient: appQueryClient,
    updateInboxState,
  });

  const bootstrappedRef = useRef(false);
  const inboxBootstrapData = inboxQuery.data;

  useEffect(() => {
    if (!inboxBootstrapData || bootstrappedRef.current) return;
    bootstrappedRef.current = true;
    updateInboxState(() => inboxBootstrapData as InboxState);
    setShellBootstrapped(true);
  }, [inboxBootstrapData, updateInboxState, setShellBootstrapped]);

  subscribeInbox({
    currentUser,
    shellBootstrapped,
    systemChannels,
    channels,
    dmChannels,
    agents,
    updateInboxState,
  });

  return (
    <div className="app-shell">
      <Sidebar />
      <MainPanel />
    </div>
  );
}

export function applyReadCursorAck(params: {
  queryClient: typeof appQueryClient;
}) {
  return (ack: ReadCursorAckPayload) => {
    (useStore as any).getState().applyReadCursorAck(ack);
    params.queryClient.invalidateQueries({ queryKey: ["inbox"] });
  };
}
