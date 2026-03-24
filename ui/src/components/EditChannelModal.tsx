import { useState } from 'react'
import { archiveChannel, deleteChannel, updateChannel } from '../api'
import type { ChannelInfo } from '../types'

interface EditChannelModalProps {
  channel: ChannelInfo
  onClose: () => void
  onSaved: (channel: { id: string; name: string; description?: string | null }) => void
}

interface DeleteChannelModalProps {
  channel: ChannelInfo
  onClose: () => void
  onArchived: () => void
  onDeleted: () => void
}

function normalizeChannelInput(name: string): string {
  return name.trim().replace(/^#/, '')
}

export function EditChannelModal({ channel, onClose, onSaved }: EditChannelModalProps) {
  const [name, setName] = useState(channel.name)
  const [description, setDescription] = useState(channel.description ?? '')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleSave() {
    const trimmed = normalizeChannelInput(name)
    if (!trimmed || !channel.id) return
    setSaving(true)
    setError(null)
    try {
      const updated = await updateChannel(channel.id, {
        name: trimmed,
        description,
      })
      onSaved(updated)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="modal-overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal-card">
        <div className="modal-header">
          <span className="modal-title">Edit Channel</span>
          <button className="modal-close" onClick={onClose}>×</button>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <div className="form-group">
          <label className="form-label">Name</label>
          <input
            className="form-input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleSave()}
            autoFocus
          />
        </div>

        <div className="form-group">
          <label className="form-label">Description (optional)</label>
          <textarea
            className="form-textarea"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </div>

        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 20 }}>
          <button className="btn-brutal" onClick={onClose}>Cancel</button>
          <button
            className="btn-brutal btn-cyan"
            onClick={handleSave}
            disabled={saving || !normalizeChannelInput(name)}
          >
            {saving ? 'Saving…' : 'Save Changes'}
          </button>
        </div>
      </div>
    </div>
  )
}

export function DeleteChannelModal({
  channel,
  onClose,
  onArchived,
  onDeleted,
}: DeleteChannelModalProps) {
  const [busyAction, setBusyAction] = useState<'archive' | 'delete' | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function handleArchive() {
    if (!channel.id) return
    setBusyAction('archive')
    setError(null)
    try {
      await archiveChannel(channel.id)
      onArchived()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusyAction(null)
    }
  }

  async function handleDelete() {
    if (!channel.id) return
    setBusyAction('delete')
    setError(null)
    try {
      await deleteChannel(channel.id)
      onDeleted()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusyAction(null)
    }
  }

  return (
    <div className="modal-overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal-card">
        <div className="modal-header">
          <div className="modal-title-block">
            <span className="modal-title">Delete Channel</span>
            <span className="modal-subtitle">#{channel.name}</span>
          </div>
          <button className="modal-close" onClick={onClose}>×</button>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <div className="modal-field-hint" style={{ marginTop: 0, marginBottom: 14 }}>
          Archive removes the channel from the sidebar but keeps its history in storage. Permanent
          delete removes the channel and its tasks, messages, and memberships.
        </div>

        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 20, flexWrap: 'wrap' }}>
          <button className="btn-brutal" onClick={onClose} disabled={busyAction !== null}>Cancel</button>
          <button
            className="btn-brutal btn-yellow"
            onClick={handleArchive}
            disabled={busyAction !== null}
          >
            {busyAction === 'archive' ? 'Archiving…' : 'Archive Channel'}
          </button>
          <button
            className="btn-brutal btn-orange"
            onClick={handleDelete}
            disabled={busyAction !== null}
          >
            {busyAction === 'delete' ? 'Deleting…' : 'Delete Permanently'}
          </button>
        </div>
      </div>
    </div>
  )
}
