import { useRef, useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import { useStore } from "./store/uiStore";
import type { ToastEntry } from "./store/uiStore";
import {
  agentsQuery,
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
  ensureInboxConversations,
  type InboxState,
} from "./store/inbox";
import { dmConversationNameForParticipants } from "./data";
import { isVisibleSidebarChannel } from "./pages/Sidebar/sidebarChannels";
import { getSession, EventType } from "./transport";
import { queryClient as appQueryClient } from "./lib/queryClient";
import { MainPanel } from "./pages/MainPanel";
import { Sidebar } from "./pages/Sidebar";
import { ToastRegion } from "./components/chat/ToastRegion";
import type { AgentInfo } from "./data";

function GlobalToasts() {
  const toasts = useStore((s) => s.toasts);
  const dismissToast = useStore((s) => s.dismissToast);
  const timers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());

  useEffect(() => {
    // Start timers for newly arrived toasts only
    for (const t of toasts) {
      if (!timers.current.has(t.id)) {
        timers.current.set(
          t.id,
          setTimeout(() => {
            dismissToast(t.id);
            timers.current.delete(t.id);
          }, 4000),
        );
      }
    }
    // Cancel timers for toasts that were dismissed early
    for (const [id, timer] of timers.current) {
      if (!toasts.find((t: ToastEntry) => t.id === id)) {
        clearTimeout(timer);
        timers.current.delete(id);
      }
    }
  }, [toasts, dismissToast]);

  return <ToastRegion toasts={toasts} onDismiss={dismissToast} />;
}

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

/** Keep inbox conversations in sync as new channels arrive from the channels query. */
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

function syncSelectedAgent(
  agents: AgentInfo[],
  setCurrentAgent: (agent: AgentInfo | null) => void,
): void {
  const currentAgent = useStore((s) => s.currentAgent);

  useEffect(() => {
    if (!currentAgent) return;
    const freshAgent =
      agents.find(
        (agent) =>
          agent.id === currentAgent.id || agent.name === currentAgent.name,
      ) ?? null;

    if (!freshAgent) {
      setCurrentAgent(null);
      return;
    }

    if (freshAgent === currentAgent) return;
    setCurrentAgent(freshAgent);
  }, [agents, currentAgent, setCurrentAgent]);
}

/** Pick the first joined channel on initial load when no channel or agent is already selected. */
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

/** Create the DM channel for the selected agent if it doesn't exist yet, then register it in the query cache and inbox state. */
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
      // Only StreamEvent frames carry MessageCreated payloads; trace and
      // task_update frames have their own dedicated subscribers and reach
      // here too because subscribeAll is wildcard. Bail early so the type
      // guard narrows `frame` to the StreamEvent branch.
      if (frame.type !== "event") return;
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
  const setCurrentAgent = useStore((s) => s.setCurrentAgent);
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
  const agents = agentsQuery.data ?? [];
  prevAllChannelsRef.current = channelsData?.allChannels;
  const allChannels = channelsData?.allChannels ?? [];
  const channels = channelsData?.channels ?? [];
  const systemChannels = channelsData?.systemChannels ?? [];
  const dmChannels = channelsData?.dmChannels ?? [];

  syncWhoami(whoamiQuery.data, currentUser, setCurrentUser, resetUserSession);
  syncSelectedAgent(agents, setCurrentAgent);

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
      <GlobalToasts />
    </div>
  );
}
