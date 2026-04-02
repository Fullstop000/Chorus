import { useMemo, useRef } from "react";
import { useStore } from "./store/uiStore";
import { loadAppData } from "./store/useAppDataQueries";
import {
  syncWhoami,
  mirrorChannels,
  autoSelectChannel,
  ensureAgentDm,
} from "./store/useShellLifecycle";
import { useAppRefreshActions } from "./store/useAppRefreshActions";
import { subscribeInbox } from "./store/useInboxRealtimeSubscription";
import { queryClient as appQueryClient } from "./lib/utils";
import type { InboxState, ChannelInfo } from "./data";
import type { ReadCursorAckPayload } from "./inbox";

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
  const {
    whoamiQuery,
    agentsQuery,
    channelsQuery,
    teamsQuery,
    humansQuery,
    inboxQuery,
  } = queries;

  const channelsData = channelsQuery.data;
  prevAllChannelsRef.current = channelsData?.allChannels;

  const agents = useMemo(() => agentsQuery.data ?? [], [agentsQuery.data]);
  const allChannels = channelsData?.allChannels ?? [];
  const channels = channelsData?.channels ?? [];
  const systemChannels = channelsData?.systemChannels ?? [];
  const dmChannels = channelsData?.dmChannels ?? [];
  const teams = teamsQuery.data ?? [];
  const humans = humansQuery.data ?? [];

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
  if (inboxBootstrapData && !bootstrappedRef.current) {
    bootstrappedRef.current = true;
    updateInboxState(() => inboxBootstrapData as InboxState);
    setShellBootstrapped(true);
  }

  const refreshActions = useAppRefreshActions({
    currentUser,
    queryClient: appQueryClient,
    setConversationThreads: (conversationId, threads) => {
      (useStore as any)
        .getState()
        .setConversationThreads(conversationId, threads);
    },
  });

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
      <MainPanel
        currentUser={currentUser}
        shellBootstrapped={shellBootstrapped}
        agents={agents}
        channels={channels}
        systemChannels={systemChannels}
        dmChannels={dmChannels}
        teams={teams}
        humans={humans}
        updateInboxState={updateInboxState}
        refreshActions={refreshActions}
      />
    </div>
  );
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const MainPanel = (_props: any) => {
  return null;
};

export function applyReadCursorAck(params: {
  queryClient: typeof appQueryClient;
}) {
  return (ack: ReadCursorAckPayload) => {
    (useStore as any).getState().applyReadCursorAck(ack);
    params.queryClient.invalidateQueries({ queryKey: ["inbox"] });
  };
}
