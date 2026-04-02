import { useQuery } from '@tanstack/react-query'
import {
  whoamiQuery,
  agentsQuery,
  channelsQuery,
  teamsQuery,
  humansQuery,
  inboxQuery,
} from '../data'

export {
  channelQueryKeys,
  agentQueryKeys,
  teamQueryKeys,
  inboxQueryKeys,
  whoamiQuery,
  agentsQuery,
  channelsQuery,
  teamsQuery,
  humansQuery,
  inboxQuery,
} from '../data'

/** Convenience hook — same queries, same shape as before. */
export function loadAppData(
  currentUser: string,
  shellBootstrapped: boolean,
  channelsData?: import('../data').ChannelInfo[]
) {
  const whoamiResult = useQuery(whoamiQuery)
  const agentsResult = useQuery(agentsQuery(currentUser))
  const channelsResult = useQuery(channelsQuery(currentUser))
  const teamsResult = useQuery(teamsQuery(currentUser))
  const humansResult = useQuery(humansQuery(currentUser))
  const inboxResult = useQuery(inboxQuery(currentUser, shellBootstrapped, channelsData))

  return {
    whoamiQuery: whoamiResult,
    agentsQuery: agentsResult,
    channelsQuery: channelsResult,
    teamsQuery: teamsResult,
    humansQuery: humansResult,
    inboxQuery: inboxResult,
  }
}

export type AppDataQueriesResult = ReturnType<typeof loadAppData>
