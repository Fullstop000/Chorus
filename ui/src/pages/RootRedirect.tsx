import { Navigate } from 'react-router-dom'
import { useStore } from '../store/uiStore'
import { useChannels } from '../hooks/data'
import { isVisibleSidebarChannel } from './Sidebar/sidebarChannels'
import { channelPath } from '../lib/routes'

/**
 * Renders at `/`. Replaces the old `autoSelectChannel` effect: once the
 * shell has bootstrapped, redirects to the first joined channel (system
 * channels first, then visible user channels). Renders nothing while the
 * bootstrap is in flight or when no channels are joined — the empty
 * state in the parent layout handles the no-channels case.
 *
 * `replace: true` is critical so back-button from the first channel does
 * not loop back to `/`.
 */
export function RootRedirect(): JSX.Element | null {
  const shellBootstrapped = useStore((s) => s.shellBootstrapped)
  const { channels, systemChannels } = useChannels()

  if (!shellBootstrapped) return null

  const joined = [
    ...systemChannels.filter((c) => c.joined),
    ...channels.filter(isVisibleSidebarChannel),
  ]
  const first = joined[0]
  if (first) return <Navigate to={channelPath(first.name)} replace />
  return null
}
