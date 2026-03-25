import { useEffect, useState } from 'react'
import { addTeamMember, deleteTeam, removeTeamMember, updateTeam } from '../api'
import { useApp } from '../store'
import type { Team, TeamMember } from '../types'
import './TeamSettings.css'

interface Props {
  team: Team
  members: TeamMember[]
  onClose: () => void
  onRefresh: () => Promise<void>
  onDeleted: () => Promise<void>
}

export function TeamSettings({ team, members, onClose, onRefresh, onDeleted }: Props) {
  const { serverInfo, agents, setSelectedChannel } = useApp()
  const [displayName, setDisplayName] = useState(team.display_name)
  const [collaborationModel, setCollaborationModel] = useState(team.collaboration_model)
  const [leaderAgentName, setLeaderAgentName] = useState(team.leader_agent_name ?? '')
  const [pendingMemberName, setPendingMemberName] = useState('')
  const [pendingMemberRole, setPendingMemberRole] = useState('operator')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setDisplayName(team.display_name)
    setCollaborationModel(team.collaboration_model)
    setLeaderAgentName(team.leader_agent_name ?? '')
  }, [team])

  const directory = [
    ...agents.map((agent) => ({
      member_name: agent.name,
      member_type: 'agent' as const,
      member_id: agent.id ?? agent.name,
      label: `${agent.display_name ?? agent.name} · agent`,
    })),
    ...(serverInfo?.humans ?? []).map((human) => ({
      member_name: human.name,
      member_type: 'human' as const,
      member_id: human.name,
      label: `${human.name} · human`,
    })),
  ]
  const availableMembers = directory.filter(
    (entry) => !members.some((member) => member.member_name === entry.member_name)
  )
  const agentMembers = members.filter((member) => member.member_type === 'agent')

  async function handleSave() {
    setSaving(true)
    setError(null)
    try {
      await updateTeam(team.name, {
        display_name: displayName.trim(),
        collaboration_model: collaborationModel,
        leader_agent_name: collaborationModel === 'swarm' ? null : leaderAgentName || null,
      })
      await onRefresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  async function handleAddMember() {
    const selected = availableMembers.find((entry) => entry.member_name === pendingMemberName)
    if (!selected) return
    setSaving(true)
    setError(null)
    try {
      await addTeamMember(team.name, {
        member_name: selected.member_name,
        member_type: selected.member_type,
        member_id: selected.member_id,
        role: pendingMemberRole.trim() || (selected.member_type === 'agent' ? 'operator' : 'observer'),
      })
      setPendingMemberName('')
      setPendingMemberRole('operator')
      await onRefresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  async function handleRemoveMember(member: TeamMember) {
    setSaving(true)
    setError(null)
    try {
      await removeTeamMember(team.name, member.member_name)
      await onRefresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  async function handleDelete() {
    if (!window.confirm(`Delete team ${team.display_name}? This archives #${team.name}.`)) {
      return
    }
    setSaving(true)
    setError(null)
    try {
      await deleteTeam(team.name)
      setSelectedChannel(null)
      await onDeleted()
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setSaving(false)
    }
  }

  return (
    <div className="modal-overlay" onClick={(event) => event.target === event.currentTarget && onClose()}>
      <div className="modal-card team-settings-card">
        <div className="modal-header">
          <div className="modal-title-block">
            <span className="modal-title">Team Settings</span>
            <span className="modal-subtitle">#{team.name}</span>
          </div>
          <button className="modal-close" onClick={onClose}>×</button>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <div className="form-group">
          <label className="form-label">Display Name</label>
          <input
            className="form-input"
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
            disabled={saving}
          />
        </div>

        <div className="form-group">
          <label className="form-label">Collaboration Model</label>
          <select
            className="form-select"
            value={collaborationModel}
            onChange={(event) =>
              setCollaborationModel(event.target.value as 'leader_operators' | 'swarm')
            }
            disabled={saving}
          >
            <option value="leader_operators">Leader+Operators</option>
            <option value="swarm">Swarm</option>
          </select>
        </div>

        {collaborationModel === 'leader_operators' && (
          <div className="form-group">
            <label className="form-label">Leader</label>
            <select
              className="form-select"
              value={leaderAgentName}
              onChange={(event) => setLeaderAgentName(event.target.value)}
              disabled={saving}
            >
              <option value="">Select an agent leader</option>
              {agentMembers.map((member) => (
                <option key={member.member_name} value={member.member_name}>
                  {member.member_name}
                </option>
              ))}
            </select>
          </div>
        )}

        <div className="form-group">
          <label className="form-label">Members</label>
          <div className="team-settings-list">
            {members.map((member) => (
              <div key={member.member_name} className="team-settings-member">
                <div className="team-settings-member-copy">
                  <div className="team-settings-member-name">{member.member_name}</div>
                  <div className="team-settings-member-meta">
                    {member.member_type} · role: {member.role}
                  </div>
                </div>
                <button
                  className="btn-brutal-sm"
                  type="button"
                  onClick={() => handleRemoveMember(member)}
                  disabled={saving}
                >
                  Remove
                </button>
              </div>
            ))}
          </div>
        </div>

        <div className="form-group">
          <label className="form-label">Add Member</label>
          <div className="team-settings-add-row">
            <select
              className="form-select"
              value={pendingMemberName}
              onChange={(event) => setPendingMemberName(event.target.value)}
              disabled={saving}
            >
              <option value="">Choose a person or agent</option>
              {availableMembers.map((member) => (
                <option key={member.member_name} value={member.member_name}>
                  {member.label}
                </option>
              ))}
            </select>
            <input
              className="form-input"
              value={pendingMemberRole}
              onChange={(event) => setPendingMemberRole(event.target.value)}
              placeholder="role"
              disabled={saving}
            />
            <button
              className="btn-brutal btn-cyan"
              type="button"
              onClick={handleAddMember}
              disabled={saving || !pendingMemberName}
            >
              Add
            </button>
          </div>
        </div>

        <div className="team-settings-actions">
          <button className="btn-brutal btn-orange" type="button" onClick={handleDelete} disabled={saving}>
            Delete Team
          </button>
          <div style={{ flex: 1 }} />
          <button className="btn-brutal" type="button" onClick={onClose} disabled={saving}>
            Close
          </button>
          <button className="btn-brutal btn-cyan" type="button" onClick={handleSave} disabled={saving}>
            Save
          </button>
        </div>
      </div>
    </div>
  )
}
