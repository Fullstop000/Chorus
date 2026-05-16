import { useLocation, useMatch, useNavigate } from 'react-router-dom'
import { channelPath, tasksBoardPath, agentTabPath, dmPath } from '../lib/routes'
import './TabBar.css'

interface Tab {
  label: string
  path: string
  isActive: boolean
}

function TabButton({ tab }: { tab: Tab }) {
  const navigate = useNavigate()
  return (
    <button
      type="button"
      onClick={() => navigate(tab.path)}
      className={`tab-bar__item${tab.isActive ? ' is-active' : ''}`}
    >
      {tab.label}
    </button>
  )
}

export function TabBar() {
  const channelMatch = useMatch('/c/:channel/*')
  const agentTabMatch = useMatch('/agent/:agent/:tab')
  const dmMatch = useMatch('/dm/:agent')
  const location = useLocation()

  if (channelMatch) {
    const name = channelMatch.params.channel
    if (!name) return null
    const chatPath = channelPath(name)
    const tasksPath = tasksBoardPath(name)
    const tabs: Tab[] = [
      { label: 'Chat', path: chatPath, isActive: location.pathname === chatPath },
      {
        label: 'Tasks',
        path: tasksPath,
        // Parent-tab highlight persists for task detail (`/tasks/:n`).
        isActive: location.pathname.startsWith(tasksPath),
      },
    ]
    return (
      <div className="tab-bar">
        {tabs.map((tab) => (
          <TabButton key={tab.label} tab={tab} />
        ))}
      </div>
    )
  }

  const agentName = agentTabMatch?.params.agent ?? dmMatch?.params.agent
  if (agentName) {
    const dmTo = dmPath(agentName)
    const tabs: Tab[] = [
      { label: 'Chat', path: dmTo, isActive: location.pathname === dmTo },
      ...(['workspace', 'activity', 'profile'] as const).map((t) => {
        const path = agentTabPath(agentName, t)
        return {
          label: t.charAt(0).toUpperCase() + t.slice(1),
          path,
          isActive: location.pathname === path,
        }
      }),
    ]
    return (
      <div className="tab-bar">
        {tabs.map((tab) => (
          <TabButton key={tab.label} tab={tab} />
        ))}
      </div>
    )
  }

  return null
}
