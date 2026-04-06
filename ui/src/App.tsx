import { useRef, useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import { useStore } from "./store/uiStore";
import {
  whoamiQuery,
  channelsQuery,
  teamsQuery,
  humansQuery,
  inboxQuery,
  ensureDirectMessageConversation,
  channelQueryKeys,
} from "./data";
import type { ChannelInfo } from "./data";
import {
  dmConversationNameForParticipants,
  ensureInboxConversations,
  type InboxState,
} from "./inbox";
import { isVisibleSidebarChannel } from "./pages/Sidebar/sidebarChannels";
import { getSession, EventType } from "./transport";
import { queryClient as appQueryClient } from "./lib/queryClient";
import { MainPanel } from "./pages/MainPanel";
import { Sidebar } from "./pages/Sidebar";

function loadAppData(
  currentUser: string,
  shellBootstrapped: boolean,
  channelsData?: import("./data").ChannelInfo[],
) {
  const whoamiResult = useQuery(whoamiQuery);
  const channelsResult = useQuery(channelsQuery(currentUser));
  const teamsResult = useQuery(teamsQuery(currentUser));
  const humansResult = useQuery(humansQuery(currentUser));
  const inboxResult = useQuery(
    inboxQuery(currentUser, shellBootstrapped, channelsData),
  );

  return {
    whoamiQuery: whoamiResult,
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

/** Advance latestSeq for ALL conversations on realtime messages so sidebar badges update. */
function useGlobalSeqListener(
  currentUser: string,
  shellBootstrapped: boolean,
): void {
  const advanceConversationLatestSeq = useStore(
    (s) => s.advanceConversationLatestSeq,
  );
  useEffect(() => {
    if (!currentUser || !shellBootstrapped) return;
    return getSession(currentUser).subscribeAll((frame) => {
      if (frame.type === "error") return;
      if (frame.event.eventType !== EventType.MessageCreated) return;
      const seq = frame.event.payload?.seq;
      if (typeof seq === "number") {
        advanceConversationLatestSeq(frame.event.channelId, seq);
      }
    });
  }, [currentUser, shellBootstrapped, advanceConversationLatestSeq]);
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
  const { whoamiQuery, channelsQuery, inboxQuery } = queries;

  const channelsData = channelsQuery.data;
  prevAllChannelsRef.current = channelsData?.allChannels;
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

  useGlobalSeqListener(currentUser, shellBootstrapped);

  return (
    <div className="app-shell">
      <Sidebar />
      <MainPanel />
    </div>
  );
}
