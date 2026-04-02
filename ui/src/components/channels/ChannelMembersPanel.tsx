import { useMemo, useState } from 'react'
import { UserPlus, X } from 'lucide-react'
import { inviteChannelMember } from '../../data'
import type { AgentInfo, ChannelMemberInfo, HumanInfo } from '../../data'
import { Dialog, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { FormField, FormError } from '@/components/ui/form'
import { Label } from '@/components/ui/label'
import './ChannelMembersPanel.css'

interface ChannelMembersPanelProps {
  channelId: string
  channelName: string
  currentUser: string
  members: ChannelMemberInfo[]
  agents: AgentInfo[]
  humans: HumanInfo[]
  invitable: boolean
  onClose: () => void
  onMembersChange: (members: ChannelMemberInfo[]) => void
}

interface InviteMemberModalProps {
  options: InviteOption[]
  channelName: string
  open: boolean
  onOpenChange: (open: boolean) => void
  onInvited: (memberName: string) => Promise<void>
}

interface InviteOption {
  memberName: string
  label: string
  detail: string
}

function memberLabel(member: ChannelMemberInfo): string {
  return member.displayName?.trim() || member.memberName
}

function InviteMemberModal({ options, channelName, open, onOpenChange, onInvited }: InviteMemberModalProps) {
  const [selectedMember, setSelectedMember] = useState(options[0]?.memberName ?? '')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleInvite() {
    if (!selectedMember) return
    setSubmitting(true)
    setError(null)
    try {
      await onInvited(selectedMember)
      onOpenChange(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <div className="flex flex-col gap-1">
            <DialogTitle>Invite Member</DialogTitle>
            <DialogDescription>#{channelName}</DialogDescription>
          </div>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>

        {error && <FormError>{error}</FormError>}

        <FormField>
          <Label>Member</Label>
          <Select value={selectedMember} onValueChange={setSelectedMember} disabled={submitting}>
            <SelectTrigger aria-label="Member">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {options.map((option) => (
                <SelectItem key={option.memberName} value={option.memberName}>
                  {option.label} · {option.detail}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </FormField>

        <p className="text-xs text-muted-foreground leading-relaxed">
          Only people and agents that are not already in this channel are shown here.
        </p>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={submitting}>Cancel</Button>
          <Button
            onClick={handleInvite}
            disabled={submitting || !selectedMember}
          >
            {submitting ? 'Inviting…' : 'Invite Member'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function ChannelMembersPanel({
  channelId,
  channelName,
  currentUser,
  members,
  agents,
  humans,
  invitable,
  onClose,
  onMembersChange,
}: ChannelMembersPanelProps) {
  const [showInviteModal, setShowInviteModal] = useState(false)
  const memberNames = useMemo(() => new Set(members.map((member) => member.memberName)), [members])
  const inviteOptions = useMemo(() => {
    const humanOptions = humans
      .filter((human) => !memberNames.has(human.name))
      .map((human) => ({
        memberName: human.name,
        label: human.name === currentUser ? `${human.name} (you)` : human.name,
        detail: 'human',
      }))
    const agentOptions = agents
      .filter((agent) => !memberNames.has(agent.name))
      .map((agent) => ({
        memberName: agent.name,
        label: agent.display_name?.trim() || agent.name,
        detail: `agent · ${agent.name}`,
      }))
    return [...humanOptions, ...agentOptions]
  }, [agents, currentUser, humans, memberNames])

  async function handleInvite(memberName: string) {
    const response = await inviteChannelMember(channelId, memberName)
    onMembersChange(response.members)
  }

  return (
    <>
      <aside className="members-panel">
        <div className="members-panel-header">
          <div className="members-panel-title-block">
            <span className="members-panel-kicker">Members</span>
            <span className="members-panel-title">{members.length}</span>
          </div>
          <div className="members-panel-actions">
            {invitable && (
              <Button
                size="sm"
                type="button"
                onClick={() => setShowInviteModal(true)}
                disabled={inviteOptions.length === 0}
              >
                <UserPlus size={13} />
                Invite
              </Button>
            )}
            <button type="button" className="members-panel-close" onClick={onClose} aria-label="Close members panel">
              <X size={15} />
            </button>
          </div>
        </div>

        <div className="members-panel-list">
          {members.map((member) => (
            <div key={member.memberName} className="members-panel-item">
              <div className={`members-panel-avatar ${member.memberType}`}>
                {(memberLabel(member)[0] ?? '?').toUpperCase()}
              </div>
              <div className="members-panel-copy">
                <div className="members-panel-name-row">
                  <span className="members-panel-name">{memberLabel(member)}</span>
                  <span className="members-panel-badge">{member.memberType}</span>
                </div>
                <span className="members-panel-meta">
                  {member.memberName === currentUser ? `${member.memberName} · you` : member.memberName}
                </span>
              </div>
            </div>
          ))}
          {members.length === 0 && <div className="members-panel-empty">No members in this channel yet.</div>}
        </div>
      </aside>

      {inviteOptions.length > 0 && (
        <InviteMemberModal
          options={inviteOptions}
          channelName={channelName}
          open={showInviteModal}
          onOpenChange={setShowInviteModal}
          onInvited={handleInvite}
        />
      )}
    </>
  )
}
