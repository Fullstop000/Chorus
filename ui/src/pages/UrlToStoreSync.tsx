import { useEffect } from 'react'
import { Navigate } from 'react-router-dom'
import { useStore } from '../store/uiStore'
import { useRouteSubject } from '../hooks/useRouteSubject'
import { useChannels } from '../hooks/data'
import { isVisibleSidebarChannel } from './Sidebar/sidebarChannels'
import { channelPath } from '../lib/routes'

/**
 * Transitional bridge: URL → `uiStore` nav fields.
 *
 * Existing components still read `currentChannel`/`currentAgent`/
 * `activeTab`/`showSettings`/`showDecisions`/`currentTaskDetail` from the
 * store. This component reads the URL via `useRouteSubject` and writes
 * those store fields so the rest of the app keeps working unchanged.
 *
 * The reverse direction is removed: components no longer call
 * `setCurrentChannel`/`setCurrentAgent`/`setShowSettings`/etc. directly.
 * Instead they navigate (via `<NavLink>` or `useNavigate`), and this
 * effect picks up the new URL on the next render.
 *
 * Once every read-side consumer has been migrated to call
 * `useRouteSubject` directly, this component and the mirrored store
 * fields can be deleted.
 */
export function UrlToStoreSync(): null {
  const subject = useRouteSubject()
  const setCurrentChannel = useStore((s) => s.setCurrentChannel)
  const setCurrentAgent = useStore((s) => s.setCurrentAgent)
  const setActiveTab = useStore((s) => s.setActiveTab)
  const setShowSettings = useStore((s) => s.setShowSettings)
  const setShowDecisions = useStore((s) => s.setShowDecisions)
  const setCurrentTaskDetail = useStore((s) => s.setCurrentTaskDetail)

  useEffect(() => {
    switch (subject.kind) {
      case 'channel': {
        setCurrentAgent(null)
        setCurrentChannel(subject.channel)
        setActiveTab(subject.view === 'tasks' ? 'tasks' : 'chat')
        setShowSettings(false)
        setShowDecisions(false)
        setCurrentTaskDetail(null)
        return
      }
      case 'task': {
        setCurrentAgent(null)
        setCurrentChannel(subject.channel)
        setActiveTab('tasks')
        setShowSettings(false)
        setShowDecisions(false)
        // Task targets require a stable channel id. If the resolved channel
        // is missing its id (shouldn't happen in practice — channels always
        // have ids once persisted), fall back to clearing the detail.
        if (subject.channel.id) {
          setCurrentTaskDetail({
            parentChannelId: subject.channel.id,
            parentSlug: subject.channel.name,
            taskNumber: subject.taskNumber,
          })
        } else {
          setCurrentTaskDetail(null)
        }
        return
      }
      case 'dm': {
        setCurrentChannel(null)
        setCurrentAgent(subject.agent)
        setActiveTab('chat')
        setShowSettings(false)
        setShowDecisions(false)
        setCurrentTaskDetail(null)
        return
      }
      case 'agent-tab': {
        setCurrentChannel(null)
        setCurrentAgent(subject.agent)
        setActiveTab(subject.tab)
        setShowSettings(false)
        setShowDecisions(false)
        setCurrentTaskDetail(null)
        return
      }
      case 'inbox': {
        setShowDecisions(true)
        setShowSettings(false)
        setCurrentTaskDetail(null)
        return
      }
      case 'settings': {
        setShowSettings(true)
        setShowDecisions(false)
        setCurrentTaskDetail(null)
        return
      }
      case 'root':
      case 'unknown':
      default:
        // Don't mutate state at /. RootRedirect handles redirect; leaving
        // store fields alone preserves the previous view while the
        // redirect resolves.
        return
    }
  }, [
    subject,
    setCurrentChannel,
    setCurrentAgent,
    setActiveTab,
    setShowSettings,
    setShowDecisions,
    setCurrentTaskDetail,
  ])

  return null
}

/**
 * Replaces the old `autoSelectChannel` effect. At `/`, redirects to the
 * first joined channel once the shell has bootstrapped. Renders nothing
 * if no channels are joined (the empty state in `MainPanel` handles that
 * via the unset currentChannel + currentAgent).
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
