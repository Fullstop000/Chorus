import { useEffect, useState } from 'react'
import { addTeamMember, deleteTeam, removeTeamMember, updateTeam } from '../api'
import { useApp } from '../store'
import type { Team, TeamMember } from '../types'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { FormField, FormError } from '@/components/ui/form'
import { Label } from '@/components/ui/label'
import './TeamSettings.css'

interface Props {
  team: Team
  members: TeamMember[]
  open: boolean
  onOpenChange: (open: boolean) => void
  onRefresh: () => Promise<void>
  onDeleted: () => Promise<void>
}

export function TeamSettings({ team, members, open, onOpenChange, onRefresh, onDeleted }: Props) {
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
      onOpenChange(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="team-settings-card">
        <DialogHeader>
          <div className="flex flex-col gap-1">
            <DialogTitle>Team Settings</DialogTitle>
            <DialogDescription>#{team.name}</DialogDescription>
          </div>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>

        {error && <FormError>{error}</FormError>}

        <FormField>
          <Label>Display Name</Label>
          <Input
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
            disabled={saving}
          />
        </FormField>

        <FormField>
          <Label>Collaboration Model</Label>
          <Select
            value={collaborationModel}
            onValueChange={(value) =>
              setCollaborationModel(value as 'leader_operators' | 'swarm')
            }
            disabled={saving}
          >
            <SelectTrigger aria-label="Collaboration Model">
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
            <Label>Leader</Label>
            <Select
              value={leaderAgentName}
              onValueChange={setLeaderAgentName}
              disabled={saving}
            >
              <SelectTrigger aria-label="Leader">
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
          <Label>Members</Label>
          <div className="team-settings-list">
            {members.map((member) => (
              <div key={member.member_name} className="team-settings-member">
                <div className="team-settings-member-copy">
                  <div className="team-settings-member-name">{member.member_name}</div>
                  <div className="team-settings-member-meta">
                    {member.member_type} · role: {member.role}
                  </div>
                </div>
                <Button
                  size="sm"
                  variant="ghost"
                  type="button"
                  onClick={() => handleRemoveMember(member)}
                  disabled={saving}
                >
                  Remove
                </Button>
              </div>
            ))}
          </div>
        </FormField>

        <FormField>
          <Label>Add Member</Label>
          <div className="team-settings-add-row">
            <Select
              value={pendingMemberName}
              onValueChange={setPendingMemberName}
              disabled={saving}
            >
              <SelectTrigger aria-label="Add Member">
                <SelectValue placeholder="Choose a person or agent" />
              </SelectTrigger>
              <SelectContent>
                {availableMembers.map((member) => (
                  <SelectItem key={member.member_name} value={member.member_name}>
                    {member.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Input
              value={pendingMemberRole}
              onChange={(event) => setPendingMemberRole(event.target.value)}
              placeholder="role"
              disabled={saving}
            />
            <Button
              type="button"
              onClick={handleAddMember}
              disabled={saving || !pendingMemberName}
            >
              Add
            </Button>
          </div>
        </FormField>

        <div className="team-settings-actions">
          <Button variant="destructive" type="button" onClick={handleDelete} disabled={saving}>
            Delete Team
          </Button>
          <div style={{ flex: 1 }} />
          <Button variant="outline" type="button" onClick={() => onOpenChange(false)} disabled={saving}>
            Close
          </Button>
          <Button type="button" onClick={handleSave} disabled={saving}>
            Save
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
