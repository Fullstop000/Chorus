import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { updateHuman, channelQueryKeys, getSystemInfo, getLogs } from '../../data'
import type { SystemInfo } from '../../data'
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

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export function UserSettings({ username, open, onOpenChange }: Props) {
  const queryClient = useQueryClient()
  const humans = useHumans()
  const currentHuman = humans.find((h) => h.name === username)

  const [displayName, setDisplayName] = useState(currentHuman?.display_name ?? '')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null)
  const [logs, setLogs] = useState<string[]>([])
  const [logsLoading, setLogsLoading] = useState(false)
  const logContainerRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (open) setDisplayName(currentHuman?.display_name ?? '')
  }, [currentHuman?.display_name, open])

  useEffect(() => {
    if (!open) return
    getSystemInfo().then(setSystemInfo).catch(() => {})
    setLogsLoading(true)
    getLogs(500).then((r) => {
      setLogs(r.lines)
      setLogsLoading(false)
    }).catch(() => setLogsLoading(false))
  }, [open])

  // Auto-scroll to bottom when logs load
  useEffect(() => {
    if (logContainerRef.current) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight
    }
  }, [logs])

  async function handleSave() {
    setSaving(true)
    setError(null)
    try {
      await updateHuman(username, { display_name: displayName.trim() || null })
      await queryClient.invalidateQueries({ queryKey: channelQueryKeys.humans })
      onOpenChange(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  function handleRefreshLogs() {
    setLogsLoading(true)
    getLogs(500).then((r) => {
      setLogs(r.lines)
      setLogsLoading(false)
    }).catch(() => setLogsLoading(false))
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

        {systemInfo && (
          <div className="user-settings-system">
            <div className="user-settings-section-label">System</div>
            <div className="user-settings-info-grid">
              <span className="user-settings-info-label">Data directory</span>
              <span className="user-settings-info-value">{systemInfo.data_dir}</span>
              <span className="user-settings-info-label">Store size</span>
              <span className="user-settings-info-value">
                {systemInfo.db_size_bytes != null ? formatBytes(systemInfo.db_size_bytes) : '—'}
              </span>
            </div>
          </div>
        )}

        <div className="user-settings-logs">
          <div className="user-settings-logs-header">
            <span className="user-settings-section-label">Server Logs</span>
            <Button
              variant="ghost"
              size="sm"
              onClick={handleRefreshLogs}
              disabled={logsLoading}
            >
              {logsLoading ? 'Loading…' : 'Refresh'}
            </Button>
          </div>
          <div className="user-settings-log-container" ref={logContainerRef}>
            {logs.length === 0 && !logsLoading && (
              <div className="user-settings-log-empty">No logs available</div>
            )}
            {logs.map((line, i) => (
              <div key={i} className="user-settings-log-line">{line}</div>
            ))}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
