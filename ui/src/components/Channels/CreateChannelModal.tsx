import React, { useEffect } from 'react'
import { Plus, Users } from 'lucide-react'
import { useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { z } from 'zod'
import { Dialog, DialogContent } from '@/components/ui/dialog'
import { Form, FormControl, FormField, FormLabel, FormMessage } from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
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

const channelSchema = z.object({
  mode: z.literal('channel'),
  name: z.string().min(1, 'Channel name is required'),
  description: z.string().optional(),
})

const teamSchema = z.object({
  mode: z.literal('team'),
  name: z.string().min(1, 'Team slug is required'),
  displayName: z.string().min(1, 'Display name is required'),
  collaborationModel: z.enum(['leader_operators', 'swarm']),
  leaderAgentName: z.string().optional(),
  pendingMemberName: z.string().optional(),
  pendingMemberRole: z.string().optional(),
})

const formSchema = z.discriminatedUnion('mode', [channelSchema, teamSchema])

type FormValues = z.infer<typeof formSchema>

export function CreateChannelModal({ open, onOpenChange, onCreated, defaultMode = 'channel' }: Props) {
  const { serverInfo, agents } = useApp()
  const [teamMembers, setTeamMembers] = React.useState<DraftTeamMember[]>([])
  const [creating, setCreating] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)

  const form = useForm<FormValues>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      mode: defaultMode,
      name: '',
      description: '',
      displayName: '',
      collaborationModel: 'leader_operators',
      leaderAgentName: '',
      pendingMemberName: '',
      pendingMemberRole: 'operator',
    },
  })

  const mode = form.watch('mode')
  const collaborationModel = form.watch('collaborationModel')
  const leaderAgentName = form.watch('leaderAgentName')

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
      form.setValue('leaderAgentName', '')
      return
    }
    if (!leaderAgentName && agentMembers.length > 0) {
      form.setValue('leaderAgentName', agentMembers[0].member_name)
    }
  }, [agentMembers, collaborationModel, leaderAgentName, form])

  useEffect(() => {
    if (open) {
      const resetValues: FormValues = defaultMode === 'channel'
        ? {
            mode: 'channel',
            name: '',
            description: '',
          }
        : {
            mode: 'team',
            name: '',
            displayName: '',
            collaborationModel: 'leader_operators',
            leaderAgentName: '',
            pendingMemberName: '',
            pendingMemberRole: 'operator',
          }
      form.reset(resetValues)
      setTeamMembers([])
      setError(null)
    }
  }, [open, defaultMode, form])

  function addTeamMemberDraft() {
    const pendingMemberName = form.getValues('pendingMemberName')
    const pendingMemberRole = form.getValues('pendingMemberRole') ?? ''
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
      form.setValue('leaderAgentName', selected.name)
    }
    form.setValue('pendingMemberName', '')
    form.setValue('pendingMemberRole', 'operator')
  }

  function removeTeamMemberDraft(memberName: string) {
    setTeamMembers((current) => current.filter((member) => member.member_name !== memberName))
    if (leaderAgentName === memberName) {
      const nextLeader = teamMembers.find(
        (member) => member.member_name !== memberName && member.member_type === 'agent'
      )
      form.setValue('leaderAgentName', nextLeader?.member_name ?? '')
    }
  }

  async function onSubmit(values: FormValues) {
    const trimmed = values.name.trim().replace(/^#/, '')
    if (!trimmed) return
    setCreating(true)
    setError(null)
    try {
      if (values.mode === 'channel') {
        const created = await createChannel({ name: trimmed, description: values.description ?? '' })
        onCreated(created)
        onOpenChange(false)
        return
      }

      const trimmedDisplayName = values.displayName.trim()
      if (!trimmedDisplayName) {
        throw new Error('Display name is required for teams')
      }
      if (values.collaborationModel === 'leader_operators' && !values.leaderAgentName) {
        throw new Error('Leader+Operators teams require a leader agent')
      }

      const created = await createTeam({
        name: trimmed,
        display_name: trimmedDisplayName,
        collaboration_model: values.collaborationModel,
        leader_agent_name: values.collaborationModel === 'swarm' ? null : values.leaderAgentName || null,
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
        </div>

        {error && <div className="error-banner">{error}</div>}

        <Form {...form}>
          <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-4">
            <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
              <Button
                type="button"
                variant={mode === 'channel' ? 'brutal' : 'outline'}
                size="sm"
                onClick={() => form.setValue('mode', 'channel')}
              >
                <Plus size={14} />
                Channel
              </Button>
              <Button
                type="button"
                variant={mode === 'team' ? 'brutal' : 'outline'}
                size="sm"
                onClick={() => form.setValue('mode', 'team')}
              >
                <Users size={14} />
                Team
              </Button>
            </div>

            <FormField
              control={form.control}
              name="name"
              render={({ field }) => (
                <div className="form-group">
                  <FormLabel>{mode === 'channel' ? 'Channel Name' : 'Team Slug'}</FormLabel>
                  <FormControl>
                    <Input
                      placeholder={mode === 'channel' ? 'e.g. engineering' : 'e.g. eng-team'}
                      {...field}
                      autoFocus
                    />
                  </FormControl>
                  <FormMessage />
                </div>
              )}
            />

            {mode === 'channel' && (
              <FormField
                control={form.control}
                name="description"
                render={({ field }) => (
                  <div className="form-group">
                    <FormLabel>Description (optional)</FormLabel>
                    <FormControl>
                      <Input
                        placeholder="What's this channel about?"
                        {...field}
                      />
                    </FormControl>
                    <FormMessage />
                  </div>
                )}
              />
            )}

            {mode === 'team' && (
              <>
                <FormField
                  control={form.control}
                  name="displayName"
                  render={({ field }) => (
                    <div className="form-group">
                      <FormLabel>Display Name</FormLabel>
                      <FormControl>
                        <Input
                          placeholder="Engineering Team"
                          {...field}
                        />
                      </FormControl>
                      <FormMessage />
                    </div>
                  )}
                />

                <FormField
                  control={form.control}
                  name="collaborationModel"
                  render={({ field }) => (
                    <div className="form-group">
                      <FormLabel>Collaboration Model</FormLabel>
                      <FormControl>
                        <select
                          className="form-select"
                          {...field}
                          onChange={(e) => field.onChange(e.target.value as 'leader_operators' | 'swarm')}
                        >
                          <option value="leader_operators">Leader+Operators</option>
                          <option value="swarm">Swarm</option>
                        </select>
                      </FormControl>
                      <FormMessage />
                    </div>
                  )}
                />

                {collaborationModel === 'leader_operators' && (
                  <FormField
                    control={form.control}
                    name="leaderAgentName"
                    render={({ field }) => (
                      <div className="form-group">
                        <FormLabel>Leader</FormLabel>
                        <FormControl>
                          <select
                            className="form-select"
                            {...field}
                          >
                            <option value="">Select an agent leader</option>
                            {agentMembers.map((member) => (
                              <option key={member.member_name} value={member.member_name}>
                                {member.member_name}
                              </option>
                            ))}
                          </select>
                        </FormControl>
                        <FormMessage />
                      </div>
                    )}
                  />
                )}

                <div className="form-group">
                  <FormLabel>Initial Members</FormLabel>
                  <div style={{ display: 'grid', gap: 8 }}>
                    <div style={{ display: 'grid', gap: 8, gridTemplateColumns: 'minmax(0, 1fr) 140px auto' }}>
                      <select
                        className="form-select"
                        value={form.getValues('pendingMemberName')}
                        onChange={(e) => form.setValue('pendingMemberName', e.target.value)}
                      >
                        <option value="">Choose a person or agent</option>
                        {availableMembers.map((member) => (
                          <option key={member.name} value={member.name}>
                            {member.label}
                          </option>
                        ))}
                      </select>
                      <Input
                        placeholder="role"
                        value={form.getValues('pendingMemberRole')}
                        onChange={(e) => form.setValue('pendingMemberRole', e.target.value)}
                      />
                      <Button
                        type="button"
                        variant="brutal"
                        size="sm"
                        onClick={addTeamMemberDraft}
                        disabled={!form.getValues('pendingMemberName')}
                      >
                        Add
                      </Button>
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
                            <Button
                              type="button"
                              variant="outline"
                              size="sm"
                              onClick={() => removeTeamMemberDraft(member.member_name)}
                            >
                              Remove
                            </Button>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                </div>
              </>
            )}

            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 20 }}>
              <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button
                type="submit"
                variant="brutal"
                disabled={
                  creating ||
                  !form.getValues('name').trim() ||
                  (mode === 'team' && !form.getValues('displayName').trim())
                }
              >
                {creating
                  ? 'Creating…'
                  : mode === 'channel'
                  ? 'Create Channel'
                  : 'Create Team'}
              </Button>
            </div>
          </form>
        </Form>
      </DialogContent>
    </Dialog>
  )
}
