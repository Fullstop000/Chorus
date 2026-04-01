import React, { useEffect, useMemo } from 'react'
import { Plus, Users } from 'lucide-react'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { createChannel, createTeam } from '../../api'
import { useApp } from '../../store'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  onCreated: (channel: { id?: string | null; name: string }) => void
  defaultMode?: 'channel' | 'team'
}

interface DraftTeamMember {
  member_name: string
  member_type: 'agent' | 'human'
  member_id: string
  role: string
}

export function CreateChannelModal({ open, onOpenChange, onCreated, defaultMode = 'channel' }: Props) {
  const { serverInfo, agents } = useApp()
  const [mode, setMode] = React.useState<'channel' | 'team'>(defaultMode)
  const [name, setName] = React.useState('')
  const [description, setDescription] = React.useState('')
  const [displayName, setDisplayName] = React.useState('')
  const [collaborationModel, setCollaborationModel] = React.useState<'leader_operators' | 'swarm'>(
    'leader_operators'
  )
  const [leaderAgentName, setLeaderAgentName] = React.useState('')
  const [teamMembers, setTeamMembers] = React.useState<DraftTeamMember[]>([])
  const [pendingMemberName, setPendingMemberName] = React.useState('')
  const [pendingMemberRole, setPendingMemberRole] = React.useState('operator')
  const [creating, setCreating] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)

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
  const agentMembers = useMemo(
    () => teamMembers.filter((member) => member.member_type === 'agent'),
    [teamMembers]
  )

  useEffect(() => {
    setMode(defaultMode)
  }, [defaultMode])

  useEffect(() => {
    if (collaborationModel === 'swarm') {
      setLeaderAgentName('')
      return
    }
    if (!leaderAgentName && agentMembers.length > 0) {
      setLeaderAgentName(agentMembers[0].member_name)
    }
  }, [agentMembers, collaborationModel, leaderAgentName])

  useEffect(() => {
    if (open) {
      setMode(defaultMode)
      setName('')
      setDescription('')
      setDisplayName('')
      setCollaborationModel('leader_operators')
      setLeaderAgentName('')
      setPendingMemberName('')
      setPendingMemberRole('operator')
      setTeamMembers([])
      setError(null)
    }
  }, [defaultMode, open])

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
        onOpenChange(false)
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
      onOpenChange(false)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setCreating(false)
    }
  }

  if (!open) return null

  return (
    <div className="modal-overlay" onClick={(e) => e.target === e.currentTarget && onOpenChange(false)}>
      <div className="modal-box">
        <div className="modal-header">
          <div className="modal-title-block">
            <span className="modal-title">{mode === 'channel' ? 'Create Channel' : 'Create Team'}</span>
            <span className="modal-subtitle">
              {mode === 'channel'
                ? 'standard room for people and agents'
                : 'shared agent collaboration unit'}
            </span>
          </div>
          <button className="modal-close" onClick={() => onOpenChange(false)}>×</button>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <form
          onSubmit={(event) => {
            event.preventDefault()
            void handleCreate()
          }}
        >
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
            <label className="form-label" htmlFor="channel-modal-name">
              {mode === 'channel' ? 'Channel Name' : 'Team Slug'}
            </label>
            <input
              id="channel-modal-name"
              className="form-input"
              placeholder={mode === 'channel' ? 'e.g. engineering' : 'e.g. eng-team'}
              value={name}
              onChange={(event) => setName(event.target.value)}
              autoFocus
            />
          </div>

          {mode === 'channel' ? (
            <div className="form-group">
              <label className="form-label" htmlFor="channel-modal-description">Description (optional)</label>
              <input
                id="channel-modal-description"
                className="form-input"
                placeholder="What's this channel about?"
                value={description}
                onChange={(event) => setDescription(event.target.value)}
              />
            </div>
          ) : (
            <>
              <div className="form-group">
                <label className="form-label" htmlFor="team-modal-display-name">Display Name</label>
                <input
                  id="team-modal-display-name"
                  className="form-input"
                  placeholder="Engineering Team"
                  value={displayName}
                  onChange={(event) => setDisplayName(event.target.value)}
                />
              </div>

              <div className="form-group">
                <label className="form-label" htmlFor="team-modal-collaboration-model">Collaboration Model</label>
                <Select
                  value={collaborationModel}
                  onValueChange={(value) => setCollaborationModel(value as 'leader_operators' | 'swarm')}
                >
                  <SelectTrigger
                    id="team-modal-collaboration-model"
                    className="form-select"
                    aria-label="Collaboration Model"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="leader_operators">Leader+Operators</SelectItem>
                    <SelectItem value="swarm">Swarm</SelectItem>
                  </SelectContent>
                </Select>
              </div>

              {collaborationModel === 'leader_operators' && (
                <div className="form-group">
                  <label className="form-label" htmlFor="team-modal-leader">Leader</label>
                  <Select value={leaderAgentName} onValueChange={setLeaderAgentName}>
                    <SelectTrigger
                      id="team-modal-leader"
                      className="form-select"
                      aria-label="Leader"
                    >
                      <SelectValue placeholder="Select an agent leader" />
                    </SelectTrigger>
                    <SelectContent>
                      {agentMembers.map((member) => (
                        <SelectItem key={member.member_name} value={member.member_name}>
                          {member.member_name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              )}

              <div className="form-group">
                <label className="form-label" htmlFor="team-modal-member">Initial Members</label>
                <div style={{ display: 'grid', gap: 8 }}>
                  <div style={{ display: 'grid', gap: 8, gridTemplateColumns: 'minmax(0, 1fr) 140px auto' }}>
                    <Select value={pendingMemberName} onValueChange={setPendingMemberName}>
                      <SelectTrigger
                        id="team-modal-member"
                        className="form-select"
                        aria-label="Initial Members"
                      >
                        <SelectValue placeholder="Choose a person or agent" />
                      </SelectTrigger>
                      <SelectContent>
                        {availableMembers.map((member) => (
                          <SelectItem key={member.name} value={member.name}>
                            {member.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <input
                      className="form-input"
                      placeholder="role"
                      value={pendingMemberRole}
                      onChange={(event) => setPendingMemberRole(event.target.value)}
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
            <button className="btn-brutal" type="button" onClick={() => onOpenChange(false)}>Cancel</button>
            <button
              className="btn-brutal btn-cyan"
              type="submit"
              disabled={creating || !name.trim() || (mode === 'team' && !displayName.trim())}
            >
              {creating
                ? 'Creating…'
                : mode === 'channel'
                ? 'Create Channel'
                : 'Create Team'}
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}
