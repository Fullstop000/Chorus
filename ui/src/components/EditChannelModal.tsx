import { useEffect, useState } from 'react'
import { archiveChannel, deleteChannel, updateChannel } from '../api'
import type { ChannelInfo } from '../types'
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'

interface EditChannelModalProps {
  channel: ChannelInfo
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved: (channel: { id: string; name: string; description?: string | null }) => void
}

interface DeleteChannelModalProps {
  channel: ChannelInfo
  open: boolean
  onOpenChange: (open: boolean) => void
  onArchived: () => void
  onDeleted: () => void
}

function normalizeChannelInput(name: string): string {
  return name.trim().replace(/^#/, '')
}

export function EditChannelModal({
  channel,
  open,
  onOpenChange,
  onSaved,
}: EditChannelModalProps) {
  const [name, setName] = useState(channel.name)
  const [description, setDescription] = useState(channel.description ?? '')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (open) {
      setName(channel.name)
      setDescription(channel.description ?? '')
      setError(null)
    }
  }, [channel.description, channel.name, open])

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
      onOpenChange(false)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <div className="modal-header">
          <DialogTitle>Edit Channel</DialogTitle>
          <DialogClose className="modal-close" aria-label="Close">×</DialogClose>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <form
          onSubmit={(event) => {
            event.preventDefault()
            void handleSave()
          }}
        >
          <div className="form-group">
            <label className="form-label" htmlFor="edit-channel-name">Name</label>
            <input
              id="edit-channel-name"
              className="form-input"
              value={name}
              onChange={(event) => setName(event.target.value)}
              autoFocus
            />
          </div>

          <div className="form-group">
            <label className="form-label" htmlFor="edit-channel-description">Description (optional)</label>
            <textarea
              id="edit-channel-description"
              className="form-textarea"
              value={description}
              onChange={(event) => setDescription(event.target.value)}
            />
          </div>

          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 20 }}>
            <button className="btn-brutal" type="button" onClick={() => onOpenChange(false)}>Cancel</button>
            <button
              className="btn-brutal btn-cyan"
              type="submit"
              disabled={saving || !normalizeChannelInput(name)}
            >
              {saving ? 'Saving…' : 'Save Changes'}
            </button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export function DeleteChannelModal({
  channel,
  open,
  onOpenChange,
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
      onOpenChange(false)
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
      onOpenChange(false)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusyAction(null)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <div className="modal-header">
          <div className="modal-title-block">
            <DialogTitle>Delete Channel</DialogTitle>
            <DialogDescription>#{channel.name}</DialogDescription>
          </div>
          <DialogClose className="modal-close" aria-label="Close">×</DialogClose>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <div className="modal-field-hint" style={{ marginTop: 0, marginBottom: 14 }}>
          Archive removes the channel from the sidebar but keeps its history in storage. Permanent
          delete removes the channel and its tasks, messages, and memberships.
        </div>

        <div
          style={{
            display: 'flex',
            justifyContent: 'flex-end',
            gap: 8,
            marginTop: 20,
            flexWrap: 'wrap',
          }}
        >
          <button className="btn-brutal" type="button" onClick={() => onOpenChange(false)} disabled={busyAction !== null}>
            Cancel
          </button>
          <button className="btn-brutal btn-yellow" type="button" onClick={handleArchive} disabled={busyAction !== null}>
            {busyAction === 'archive' ? 'Archiving…' : 'Archive Channel'}
          </button>
          <button className="btn-brutal btn-orange" type="button" onClick={handleDelete} disabled={busyAction !== null}>
            {busyAction === 'delete' ? 'Deleting…' : 'Delete Permanently'}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
