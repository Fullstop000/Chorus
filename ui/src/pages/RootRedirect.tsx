import { Navigate } from 'react-router-dom'
import { useStore } from '../store/uiStore'
import { useChannels } from '../hooks/data'
import { isVisibleSidebarChannel } from './Sidebar/sidebarChannels'
import { channelPath } from '../lib/routes'

/**
 * Renders at `/`. Replaces the old `autoSelectChannel` effect: once the
 * shell has bootstrapped, redirects to the first joined channel (system
 * channels first, then visible user channels). When no channels are
 * joined, renders an empty-state panel rather than returning null —
 * otherwise the user lands on a blank screen with only the sidebar.
 *
 * `replace: true` on the redirect is critical so the browser back
 * button from the first channel does not loop back to `/`.
 */
export function RootRedirect(): JSX.Element {
  const shellBootstrapped = useStore((s) => s.shellBootstrapped)
  const { channels, systemChannels } = useChannels()

  if (!shellBootstrapped) {
    return <EmptyShell label="Loading…" />
  }

  const joined = [
    ...systemChannels.filter((c) => c.joined),
    ...channels.filter(isVisibleSidebarChannel),
  ]
  const first = joined[0]
  if (first) return <Navigate to={channelPath(first.name)} replace />
  return <EmptyShell label="Select a channel or agent to get started" />
}

function EmptyShell({ label }: { label: string }): JSX.Element {
  return (
    <div
      className="empty-state"
      style={{
        flex: 1,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        flexDirection: 'column',
        gap: 8,
        color: 'var(--color-muted-foreground)',
      }}
    >
      <h1 className="sr-only">Chorus — {label}</h1>
      <span className="empty-state-icon">[chorus::idle]</span>
      <span>{label}</span>
    </div>
  )
}
