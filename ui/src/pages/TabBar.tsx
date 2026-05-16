import { NavLink, useMatch } from 'react-router-dom'
import { channelPath, tasksBoardPath, agentTabPath, dmPath } from '../lib/routes'
import './TabBar.css'

const tabClass = ({ isActive }: { isActive: boolean }) =>
  `tab-bar__item${isActive ? ' is-active' : ''}`

export function TabBar() {
  const channelMatch = useMatch('/c/:channel/*')
  const agentTabMatch = useMatch('/agent/:agent/:tab')
  const dmMatch = useMatch('/dm/:agent')

  if (channelMatch) {
    const name = channelMatch.params.channel
    if (!name) return null
    return (
      <div className="tab-bar">
        <NavLink to={channelPath(name)} end className={tabClass}>
          Chat
        </NavLink>
        <NavLink to={tasksBoardPath(name)} className={tabClass}>
          Tasks
        </NavLink>
      </div>
    )
  }

  const agentName = agentTabMatch?.params.agent ?? dmMatch?.params.agent
  if (agentName) {
    return (
      <div className="tab-bar">
        <NavLink to={dmPath(agentName)} end className={tabClass}>
          Chat
        </NavLink>
        <NavLink to={agentTabPath(agentName, 'workspace')} className={tabClass}>
          Workspace
        </NavLink>
        <NavLink to={agentTabPath(agentName, 'activity')} className={tabClass}>
          Activity
        </NavLink>
        <NavLink to={agentTabPath(agentName, 'profile')} className={tabClass}>
          Profile
        </NavLink>
      </div>
    )
  }

  return null
}
