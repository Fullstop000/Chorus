import { useEffect, useState } from 'react'
import { useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { z } from 'zod'
import { archiveChannel, deleteChannel, updateChannel } from '../api'
import type { ChannelInfo } from '../types'
import { Dialog, DialogContent } from '@/components/ui/dialog'
import { Form, FormControl, FormField, FormLabel, FormMessage } from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'

const editChannelSchema = z.object({
  name: z.string().min(1, 'Channel name is required'),
  description: z.string().optional(),
})

type EditChannelFormValues = z.infer<typeof editChannelSchema>

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

export function EditChannelModal({
  channel,
  open,
  onOpenChange,
  onSaved,
}: EditChannelModalProps) {
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const form = useForm<EditChannelFormValues>({
    resolver: zodResolver(editChannelSchema),
    defaultValues: {
      name: channel.name,
      description: channel.description ?? '',
    },
  })

  useEffect(() => {
    if (open) {
      form.reset({
        name: channel.name,
        description: channel.description ?? '',
      })
      setError(null)
    }
  }, [open, channel.name, channel.description, form])

  async function onSubmit(values: EditChannelFormValues) {
    const trimmed = values.name.trim().replace(/^#/, '')
    if (!trimmed || !channel.id) return
    setSaving(true)
    setError(null)
    try {
      const updated = await updateChannel(channel.id, {
        name: trimmed,
        description: values.description ?? '',
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
          <div className="modal-title-block">
            <span className="modal-title">Edit Channel</span>
          </div>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <Form {...form}>
          <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-4">
            <FormField
              control={form.control}
              name="name"
              render={({ field }) => (
                <div className="form-group">
                  <FormLabel>Name</FormLabel>
                  <FormControl>
                    <Input {...field} autoFocus />
                  </FormControl>
                  <FormMessage />
                </div>
              )}
            />

            <FormField
              control={form.control}
              name="description"
              render={({ field }) => (
                <div className="form-group">
                  <FormLabel>Description (optional)</FormLabel>
                  <FormControl>
                    <Input placeholder="What's this channel about?" {...field} />
                  </FormControl>
                  <FormMessage />
                </div>
              )}
            />

            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 20 }}>
              <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button
                type="submit"
                variant="brutal"
                disabled={saving || !form.getValues('name').trim()}
              >
                {saving ? 'Saving…' : 'Save Changes'}
              </Button>
            </div>
          </form>
        </Form>
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
            <span className="modal-title">Delete Channel</span>
            <span className="modal-subtitle">#{channel.name}</span>
          </div>
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
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={busyAction !== null}
          >
            Cancel
          </Button>
          <Button
            type="button"
            variant="brutal"
            onClick={handleArchive}
            disabled={busyAction !== null}
          >
            {busyAction === 'archive' ? 'Archiving…' : 'Archive Channel'}
          </Button>
          <Button
            type="button"
            variant="brutal"
            onClick={handleDelete}
            disabled={busyAction !== null}
          >
            {busyAction === 'delete' ? 'Deleting…' : 'Delete Permanently'}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
