import { useMemo, useState } from 'react'
import { UserPlus, X } from 'lucide-react'
import { inviteChannelMember } from '../api'
import type { AgentInfo, ChannelMemberInfo, HumanInfo } from '../types'
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
  onClose: () => void
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

function InviteMemberModal({ options, channelName, onClose, onInvited }: InviteMemberModalProps) {
  const [selectedMember, setSelectedMember] = useState(options[0]?.memberName ?? '')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleInvite() {
    if (!selectedMember) return
    setSubmitting(true)
    setError(null)
    try {
      await onInvited(selectedMember)
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="modal-overlay" onClick={(event) => event.target === event.currentTarget && onClose()}>
      <div className="modal-card">
        <div className="modal-header">
          <div className="modal-title-block">
            <span className="modal-title">Invite Member</span>
            <span className="modal-subtitle">#{channelName}</span>
          </div>
          <button className="modal-close" onClick={onClose}>×</button>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <div className="form-group">
          <label className="form-label">Member</label>
          <select
            className="form-select"
            value={selectedMember}
            onChange={(event) => setSelectedMember(event.target.value)}
            disabled={submitting}
            autoFocus
          >
            {options.map((option) => (
              <option key={option.memberName} value={option.memberName}>
                {option.label} · {option.detail}
              </option>
            ))}
          </select>
        </div>

        <div className="modal-field-hint">
          Only people and agents that are not already in this channel are shown here.
        </div>

        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 20 }}>
          <button className="btn-brutal" onClick={onClose} disabled={submitting}>Cancel</button>
          <button
            className="btn-brutal btn-cyan"
            onClick={handleInvite}
            disabled={submitting || !selectedMember}
          >
            {submitting ? 'Inviting…' : 'Invite Member'}
          </button>
        </div>
      </div>
    </div>
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
              <button
                type="button"
                className="btn-brutal-sm"
                onClick={() => setShowInviteModal(true)}
                disabled={inviteOptions.length === 0}
              >
                <UserPlus size={13} />
                Invite
              </button>
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

      {showInviteModal && inviteOptions.length > 0 && (
        <InviteMemberModal
          options={inviteOptions}
          channelName={channelName}
          onClose={() => setShowInviteModal(false)}
          onInvited={handleInvite}
        />
      )}
    </>
  )
}
