import { useEffect, useState } from 'react'
import { Plus, Users } from 'lucide-react'
import { createChannel, createTeam } from '../api'
import { useApp } from '../store'

interface Props {
  onClose: () => void
  onCreated: (channel: { id?: string | null; name: string }) => void
  defaultMode?: 'channel' | 'team'
}

interface DraftTeamMember {
  member_name: string
  member_type: 'agent' | 'human'
  member_id: string
  role: string
}

export function CreateChannelModal({ onClose, onCreated, defaultMode = 'channel' }: Props) {
  const { serverInfo, agents } = useApp()
  const [mode, setMode] = useState<'channel' | 'team'>(defaultMode)
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [collaborationModel, setCollaborationModel] = useState<'leader_operators' | 'swarm'>(
    'leader_operators'
  )
  const [leaderAgentName, setLeaderAgentName] = useState('')
  const [teamMembers, setTeamMembers] = useState<DraftTeamMember[]>([])
  const [pendingMemberName, setPendingMemberName] = useState('')
  const [pendingMemberRole, setPendingMemberRole] = useState('operator')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setMode(defaultMode)
  }, [defaultMode])

  const directory = [
    ...agents.map((agent) => ({
      name: agent.name,
      member_type: 'agent' as const,
      member_id: agent.id ?? agent.name,
      label: `${agent.display_name ?? agent.name} · agent`,
    })),
    ...(serverInfo?.humans ?? []).map((human) => ({
      name: human.name,
      member_type: 'human' as const,
      member_id: human.name,
      label: `${human.name} · human`,
    })),
  ]
  const availableMembers = directory.filter(
    (member) => !teamMembers.some((entry) => entry.member_name === member.name)
  )
  const agentMembers = teamMembers.filter((member) => member.member_type === 'agent')

  useEffect(() => {
    if (collaborationModel === 'swarm') {
      setLeaderAgentName('')
      return
    }
    if (!leaderAgentName && agentMembers.length > 0) {
      setLeaderAgentName(agentMembers[0].member_name)
    }
  }, [agentMembers, collaborationModel, leaderAgentName])

  function addTeamMemberDraft() {
    const selected = availableMembers.find((member) => member.name === pendingMemberName)
    if (!selected) return
    setTeamMembers((current) => [
      ...current,
      {
        member_name: selected.name,
        member_type: selected.member_type,
        member_id: selected.member_id,
        role: pendingMemberRole.trim() || (selected.member_type === 'agent' ? 'operator' : 'observer'),
      },
    ])
    if (collaborationModel === 'leader_operators' && selected.member_type === 'agent' && !leaderAgentName) {
      setLeaderAgentName(selected.name)
    }
    setPendingMemberName('')
    setPendingMemberRole('operator')
  }

  function removeTeamMemberDraft(memberName: string) {
    setTeamMembers((current) => current.filter((member) => member.member_name !== memberName))
    if (leaderAgentName === memberName) {
      const nextLeader = teamMembers.find(
        (member) => member.member_name !== memberName && member.member_type === 'agent'
      )
      setLeaderAgentName(nextLeader?.member_name ?? '')
    }
  }

  async function handleCreate() {
    const trimmed = name.trim().replace(/^#/, '')
    if (!trimmed) return
    setCreating(true)
    setError(null)
    try {
      if (mode === 'channel') {
        const created = await createChannel({ name: trimmed, description })
        onCreated(created)
        return
      }

      const trimmedDisplayName = displayName.trim()
      if (!trimmedDisplayName) {
        throw new Error('Display name is required for teams')
      }
      if (collaborationModel === 'leader_operators' && !leaderAgentName) {
        throw new Error('Leader+Operators teams require a leader agent')
      }

      const created = await createTeam({
        name: trimmed,
        display_name: trimmedDisplayName,
        collaboration_model: collaborationModel,
        leader_agent_name: collaborationModel === 'swarm' ? null : leaderAgentName || null,
        members: teamMembers,
      })
      onCreated({ id: created.team.channel_id ?? undefined, name: created.team.name })
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setCreating(false)
    }
  }

  return (
    <div className="modal-overlay" onClick={(e) => { if (e.target === e.currentTarget) onClose() }}>
      <div className="modal-card">
        <div className="modal-header">
          <div className="modal-title-block">
            <span className="modal-title">
              {mode === 'channel' ? 'Create Channel' : 'Create Team'}
            </span>
            <span className="modal-subtitle">
              {mode === 'channel'
                ? 'standard room for people and agents'
                : 'shared agent collaboration unit'}
            </span>
          </div>
          <button className="modal-close" onClick={onClose}>×</button>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
          <button
            className={`btn-brutal${mode === 'channel' ? ' btn-cyan' : ''}`}
            type="button"
            onClick={() => setMode('channel')}
          >
            <Plus size={14} />
            Channel
          </button>
          <button
            className={`btn-brutal${mode === 'team' ? ' btn-cyan' : ''}`}
            type="button"
            onClick={() => setMode('team')}
          >
            <Users size={14} />
            Team
          </button>
        </div>

        <div className="form-group">
          <label className="form-label">{mode === 'channel' ? 'Channel Name' : 'Team Slug'}</label>
          <input
            className="form-input"
            placeholder={mode === 'channel' ? 'e.g. engineering' : 'e.g. eng-team'}
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
            autoFocus
          />
        </div>

        {mode === 'channel' ? (
          <div className="form-group">
            <label className="form-label">Description (optional)</label>
            <input
              className="form-input"
              placeholder="What's this channel about?"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
        ) : (
          <>
            <div className="form-group">
              <label className="form-label">Display Name</label>
              <input
                className="form-input"
                placeholder="Engineering Team"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
              />
            </div>

            <div className="form-group">
              <label className="form-label">Collaboration Model</label>
              <select
                className="form-select"
                value={collaborationModel}
                onChange={(e) =>
                  setCollaborationModel(e.target.value as 'leader_operators' | 'swarm')
                }
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
                  onChange={(e) => setLeaderAgentName(e.target.value)}
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
              <label className="form-label">Initial Members</label>
              <div style={{ display: 'grid', gap: 8 }}>
                <div style={{ display: 'grid', gap: 8, gridTemplateColumns: 'minmax(0, 1fr) 140px auto' }}>
                  <select
                    className="form-select"
                    value={pendingMemberName}
                    onChange={(e) => setPendingMemberName(e.target.value)}
                  >
                    <option value="">Choose a person or agent</option>
                    {availableMembers.map((member) => (
                      <option key={member.name} value={member.name}>
                        {member.label}
                      </option>
                    ))}
                  </select>
                  <input
                    className="form-input"
                    placeholder="role"
                    value={pendingMemberRole}
                    onChange={(e) => setPendingMemberRole(e.target.value)}
                  />
                  <button
                    className="btn-brutal btn-cyan"
                    type="button"
                    onClick={addTeamMemberDraft}
                    disabled={!pendingMemberName}
                  >
                    Add
                  </button>
                </div>

                {teamMembers.length === 0 ? (
                  <div className="modal-field-hint">No initial members yet.</div>
                ) : (
                  <div style={{ display: 'grid', gap: 8 }}>
                    {teamMembers.map((member) => (
                      <div
                        key={member.member_name}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          gap: 8,
                          padding: '10px 12px',
                          border: 'var(--border)',
                          background: 'var(--bg-panel-muted)',
                        }}
                      >
                        <div style={{ minWidth: 0, flex: 1 }}>
                          <div style={{ fontWeight: 600 }}>{member.member_name}</div>
                          <div className="modal-field-hint">
                            {member.member_type} · role: {member.role}
                          </div>
                        </div>
                        <button
                          className="btn-brutal-sm"
                          type="button"
                          onClick={() => removeTeamMemberDraft(member.member_name)}
                        >
                          Remove
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
          </>
        )}

        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 20 }}>
          <button className="btn-brutal" onClick={onClose}>Cancel</button>
          <button
            className="btn-brutal btn-cyan"
            onClick={handleCreate}
            disabled={
              creating ||
              !name.trim() ||
              (mode === 'team' && !displayName.trim())
            }
          >
            {creating
              ? 'Creating…'
              : mode === 'channel'
              ? 'Create Channel'
              : 'Create Team'}
          </button>
        </div>
      </div>
    </div>
  )
}
