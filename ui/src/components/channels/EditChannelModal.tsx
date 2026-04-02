import { useEffect, useState } from 'react'
import { archiveChannel, deleteChannel, updateChannel } from '../../api'
import type { ChannelInfo } from '../../types'
import { Dialog, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Label } from '@/components/ui/label'
import { FormField, FormError } from '@/components/ui/form'

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
        <DialogHeader>
          <DialogTitle>Edit Channel</DialogTitle>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>

        {error && <FormError>{error}</FormError>}

        <form
          onSubmit={(event) => {
            event.preventDefault()
            void handleSave()
          }}
        >
          <FormField>
            <Label htmlFor="edit-channel-name">Name</Label>
            <Input
              id="edit-channel-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              autoFocus
            />
          </FormField>

          <FormField>
            <Label htmlFor="edit-channel-description">Description (optional)</Label>
            <Textarea
              id="edit-channel-description"
              value={description}
              onChange={(event) => setDescription(event.target.value)}
            />
          </FormField>

          <DialogFooter>
            <Button variant="outline" type="button" onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button
              type="submit"
              disabled={saving || !normalizeChannelInput(name)}
            >
              {saving ? 'Saving…' : 'Save Changes'}
            </Button>
          </DialogFooter>
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
        <DialogHeader>
          <div className="flex flex-col gap-1">
            <DialogTitle>Delete Channel</DialogTitle>
            <DialogDescription>#{channel.name}</DialogDescription>
          </div>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>

        {error && <FormError>{error}</FormError>}

        <p className="text-xs text-muted-foreground leading-relaxed" style={{ marginBottom: 14 }}>
          Archive removes the channel from the sidebar but keeps its history in storage. Permanent
          delete removes the channel and its tasks, messages, and memberships.
        </p>

        <DialogFooter style={{ flexWrap: 'wrap' }}>
          <Button variant="outline" type="button" onClick={() => onOpenChange(false)} disabled={busyAction !== null}>
            Cancel
          </Button>
          <Button variant="outline" type="button" onClick={handleArchive} disabled={busyAction !== null}>
            {busyAction === 'archive' ? 'Archiving…' : 'Archive Channel'}
          </Button>
          <Button variant="destructive" type="button" onClick={handleDelete} disabled={busyAction !== null}>
            {busyAction === 'delete' ? 'Deleting…' : 'Delete Permanently'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
