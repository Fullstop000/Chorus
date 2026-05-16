import { useRef, useEffect } from "react";
import { Routes, Route } from "react-router-dom";
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
import { getSession, EventType } from "./transport";
import { queryClient as appQueryClient } from "./lib/queryClient";
import { MainPanel } from "./pages/MainPanel";
import { Sidebar } from "./pages/Sidebar";
import { ToastRegion } from "./components/chat/ToastRegion";
import { getHealth } from "./data";
import { useCurrentAgent } from "./hooks/useRouteSubject";
import { RootRedirect } from "./pages/RootRedirect";

function DevAuthBanner() {
  const { data } = useQuery({
    queryKey: ["health"],
    queryFn: () => getHealth(),
    refetchInterval: 60_000,
    staleTime: 60_000,
  });
  if (!data?.dev_auth) return null;
  return (
    <div className="dev-auth-banner" role="alert">
      Dev auth enabled — access-controlled by network reachability only.
    </div>
  );
}

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
  currentHumanId: string,
  shellBootstrapped: boolean,
  channelsData?: import("./data").ChannelInfo[],
) {
  const whoamiResult = useQuery(whoamiQuery);
  const agentsResult = useQuery(agentsQuery(currentHumanId));
  const channelsResult = useQuery(channelsQuery(currentHumanId));
  const teamsResult = useQuery(teamsQuery(currentHumanId));
  const humansResult = useQuery(humansQuery(currentHumanId));
  const inboxResult = useQuery(
    inboxQuery(currentHumanId, shellBootstrapped, channelsData),
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
  whoami: { id: string; name: string } | undefined,
  currentUserId: string,
  setCurrentUser: (identity: { id: string; name: string }) => void,
  resetUserSession: () => void,
): void {
  useEffect(() => {
    if (!whoami) return;
    if (whoami.id === currentUserId) return;
    // Switching local human identity invalidates the cached selection state
    // (current channel/agent, inbox cursors). Reset before adopting the new id
    // so stale ids/cursors from the previous session don't leak in.
    if (currentUserId) {
      resetUserSession();
      // Wipe the React Query cache so stale API data (channels, agents,
      // messages) from the previous human session doesn't persist.
      appQueryClient.clear();
      // Re-seed whoami so the query that just resolved doesn't flicker into
      // a loading state while components re-mount.
      appQueryClient.setQueryData(channelQueryKeys.whoami, whoami);
    }
    setCurrentUser(whoami);
  }, [whoami, currentUserId, setCurrentUser, resetUserSession]);
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

/** Create the DM channel for the selected agent if it doesn't exist yet, then register it in the query cache and inbox state. */
function ensureAgentDm(params: {
  currentHumanId: string;
  currentAgentId: string | null;
  currentAgentName: string | null;
  dmChannels: ChannelInfo[];
  queryClient: QueryClient;
  updateInboxState: (u: (c: InboxState) => InboxState) => void;
}): void {
  const {
    currentHumanId,
    currentAgentId,
    currentAgentName,
    dmChannels,
    queryClient,
    updateInboxState,
  } = params;

  useEffect(() => {
    if (!currentHumanId || !currentAgentId || !currentAgentName) return;
    const dmName = dmConversationNameForParticipants(
      currentHumanId,
      currentAgentId,
    );
    if (dmChannels.some((ch: ChannelInfo) => ch.name === dmName)) return;

    let cancelled = false;
    ensureDirectMessageConversation(currentAgentId)
      .then((channel) => {
        if (cancelled) return;
        queryClient.setQueryData<ChannelInfo[]>(
          channelQueryKeys.channels(currentHumanId),
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
    currentHumanId,
    currentAgentId,
    dmChannels,
    currentAgentName,
    queryClient,
    updateInboxState,
  ]);
}

/** Advance latestSeq for ALL conversations on realtime messages so sidebar badges update. */
function useGlobalSeqListener(
  currentHumanId: string,
  shellBootstrapped: boolean,
): void {
  const advanceConversationLatestSeq = useStore(
    (s) => s.advanceConversationLatestSeq,
  );
  useEffect(() => {
    if (!currentHumanId || !shellBootstrapped) return;
    return getSession(currentHumanId).subscribeAll((frame) => {
      if (frame.type === "error") return;
      if (frame.event.eventType === EventType.MessageCreated) {
        const seq = frame.event.payload?.seq;
        if (typeof seq === "number") {
          advanceConversationLatestSeq(frame.event.channelId, seq);
        }
      }
      if (frame.event.eventType === EventType.ChannelMemberJoined) {
        appQueryClient.invalidateQueries({
          queryKey: channelQueryKeys.members(frame.event.channelId),
        });
      }
    });
  }, [currentHumanId, shellBootstrapped, advanceConversationLatestSeq]);
}

export default function App() {
  const currentUserId = useStore((s) => s.currentUserId);
  const shellBootstrapped = useStore((s) => s.shellBootstrapped);
  const setCurrentUser = useStore((s) => s.setCurrentUser);
  const resetUserSession = useStore((s) => s.resetUserSession);
  const setShellBootstrapped = useStore((s) => s.setShellBootstrapped);
  const updateInboxState = useStore((s) => s.updateInboxState);

  const prevAllChannelsRef = useRef<ChannelInfo[] | undefined>(undefined);

  const queries = loadAppData(
    currentUserId,
    shellBootstrapped,
    prevAllChannelsRef.current,
  );
  const { whoamiQuery, channelsQuery, inboxQuery } = queries;

  const channelsData = channelsQuery.data;
  prevAllChannelsRef.current = channelsData?.allChannels;
  const allChannels = channelsData?.allChannels ?? [];
  const dmChannels = channelsData?.dmChannels ?? [];

  syncWhoami(whoamiQuery.data, currentUserId, setCurrentUser, resetUserSession);

  mirrorChannels(allChannels, updateInboxState);

  const currentAgent = useCurrentAgent();
  ensureAgentDm({
    currentHumanId: currentUserId,
    currentAgentId: currentAgent?.id ?? null,
    currentAgentName: currentAgent?.name ?? null,
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

  useGlobalSeqListener(currentUserId, shellBootstrapped);

  return (
    <div className="app-shell">
      <DevAuthBanner />
      <Sidebar />
      <Routes>
        <Route path="/" element={<RootRedirect />} />
        <Route path="*" element={<MainPanel />} />
      </Routes>
      <GlobalToasts />
    </div>
  );
}
