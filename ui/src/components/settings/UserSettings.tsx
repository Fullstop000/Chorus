import { useEffect, useState } from 'react'
import { updateHuman } from '../../data'
import { useHumans } from '../../hooks/data'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogClose,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField, FormError } from '@/components/ui/form'
import { Label } from '@/components/ui/label'
import './UserSettings.css'

interface Props {
  username: string
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function UserSettings({ username, open, onOpenChange }: Props) {
  const humans = useHumans()
  const currentHuman = humans.find((h) => h.name === username)

  const [displayName, setDisplayName] = useState(currentHuman?.display_name ?? '')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setDisplayName(currentHuman?.display_name ?? '')
  }, [currentHuman?.display_name])

  async function handleSave() {
    setSaving(true)
    setError(null)
    try {
      await updateHuman(username, { display_name: displayName.trim() || undefined })
      onOpenChange(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="user-settings-card">
        <DialogHeader>
          <DialogTitle>User Settings</DialogTitle>
          <DialogDescription>
            Logged in as <span style={{ fontFamily: 'var(--font-mono)' }}>{username}</span>
          </DialogDescription>
        </DialogHeader>

        <FormField>
          <Label htmlFor="display-name">Display name</Label>
          <Input
            id="display-name"
            placeholder={username}
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            disabled={saving}
            autoFocus
          />
        </FormField>

        {error && <FormError>{error}</FormError>}

        <div className="user-settings-actions">
          <Button onClick={handleSave} disabled={saving}>
            {saving ? 'Saving…' : 'Save'}
          </Button>
          <DialogClose asChild>
            <Button variant="ghost">Cancel</Button>
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
  )
}
