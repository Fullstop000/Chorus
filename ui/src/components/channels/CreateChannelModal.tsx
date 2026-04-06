import React, { useEffect, useMemo } from 'react'
import { Plus, Users } from 'lucide-react'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { FormField, FormError } from '@/components/ui/form'
import { Label } from '@/components/ui/label'
import { createChannel, createTeam } from '../../data'
import type { TeamMember } from '../../data'
import { useAgents, useHumans } from '../../hooks/data'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  onCreated: (channel: Pick<import('../../data').ChannelInfo, 'id' | 'name'>) => void
  defaultMode?: 'channel' | 'team'
}

type DraftTeamMember = Omit<TeamMember, 'team_id' | 'joined_at'>

export function CreateChannelModal({ open, onOpenChange, onCreated, defaultMode = 'channel' }: Props) {
  const agents = useAgents()
  const humans = useHumans()
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
    ...humans.map((human) => ({
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

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <div className="flex flex-col gap-1">
            <DialogTitle>{mode === 'channel' ? 'Create Channel' : 'Create Team'}</DialogTitle>
            <DialogDescription>
              {mode === 'channel'
                ? 'standard room for people and agents'
                : 'shared agent collaboration unit'}
            </DialogDescription>
          </div>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>

        {error && <FormError>{error}</FormError>}

        <form
          onSubmit={(event) => {
            event.preventDefault()
            void handleCreate()
          }}
        >
          <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
            <Button
              variant={mode === 'channel' ? 'default' : 'outline'}
              type="button"
              onClick={() => setMode('channel')}
            >
              <Plus size={14} />
              Channel
            </Button>
            <Button
              variant={mode === 'team' ? 'default' : 'outline'}
              type="button"
              onClick={() => setMode('team')}
            >
              <Users size={14} />
              Team
            </Button>
          </div>

          <FormField>
            <Label htmlFor="channel-modal-name">
              {mode === 'channel' ? 'Channel Name' : 'Team Slug'}
            </Label>
            <Input
              id="channel-modal-name"
              placeholder={mode === 'channel' ? 'e.g. engineering' : 'e.g. eng-team'}
              value={name}
              onChange={(event) => setName(event.target.value)}
              autoFocus
            />
          </FormField>

          {mode === 'channel' ? (
            <FormField>
              <Label htmlFor="channel-modal-description">Description (optional)</Label>
              <Input
                id="channel-modal-description"
                placeholder="What's this channel about?"
                value={description}
                onChange={(event) => setDescription(event.target.value)}
              />
            </FormField>
          ) : (
            <>
              <FormField>
                <Label htmlFor="team-modal-display-name">Display Name</Label>
                <Input
                  id="team-modal-display-name"
                  placeholder="Engineering Team"
                  value={displayName}
                  onChange={(event) => setDisplayName(event.target.value)}
                />
              </FormField>

              <FormField>
                <Label htmlFor="team-modal-collaboration-model">Collaboration Model</Label>
                <Select
                  value={collaborationModel}
                  onValueChange={(value) => setCollaborationModel(value as 'leader_operators' | 'swarm')}
                >
                  <SelectTrigger
                    id="team-modal-collaboration-model"
                    aria-label="Collaboration Model"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="leader_operators">Leader+Operators</SelectItem>
                    <SelectItem value="swarm">Swarm</SelectItem>
                  </SelectContent>
                </Select>
              </FormField>

              {collaborationModel === 'leader_operators' && (
                <FormField>
                  <Label htmlFor="team-modal-leader">Leader</Label>
                  <Select value={leaderAgentName} onValueChange={setLeaderAgentName}>
                    <SelectTrigger
                      id="team-modal-leader"
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
                </FormField>
              )}

              <FormField>
                <Label htmlFor="team-modal-member">Initial Members</Label>
                <div style={{ display: 'grid', gap: 8 }}>
                  <div style={{ display: 'grid', gap: 8, gridTemplateColumns: 'minmax(0, 1fr) 140px auto' }}>
                    <Select value={pendingMemberName} onValueChange={setPendingMemberName}>
                      <SelectTrigger
                        id="team-modal-member"
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
                    <Input
                      placeholder="role"
                      value={pendingMemberRole}
                      onChange={(event) => setPendingMemberRole(event.target.value)}
                    />
                    <Button
                      type="button"
                      onClick={addTeamMemberDraft}
                      disabled={!pendingMemberName}
                    >
                      Add
                    </Button>
                  </div>

                  {teamMembers.length === 0 ? (
                    <p className="text-xs text-muted-foreground leading-relaxed">No initial members yet.</p>
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
                            border: '1px solid var(--color-border)',
                            background: 'var(--color-muted)',
                          }}
                        >
                          <div style={{ minWidth: 0, flex: 1 }}>
                            <div style={{ fontWeight: 600 }}>{member.member_name}</div>
                            <p className="text-xs text-muted-foreground leading-relaxed">
                              {member.member_type} · role: {member.role}
                            </p>
                          </div>
                          <Button
                            size="sm"
                            variant="ghost"
                            type="button"
                            onClick={() => removeTeamMemberDraft(member.member_name)}
                          >
                            Remove
                          </Button>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </FormField>
            </>
          )}

          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 20 }}>
            <Button variant="outline" type="button" onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button
              type="submit"
              disabled={creating || !name.trim() || (mode === 'team' && !displayName.trim())}
            >
              {creating
                ? 'Creating…'
                : mode === 'channel'
                ? 'Create Channel'
                : 'Create Team'}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  )
}
