import { useState, useEffect } from 'react'
import { useRuntimeStatuses } from '../../hooks/useRuntimeStatuses'
import { AgentConfigForm, type AgentConfigState } from './AgentConfigForm'
import { Dialog, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { FormField, FormError } from '@/components/ui/form'
import { Label } from '@/components/ui/label'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  onCreated: () => void
}

const RUNTIME_ORDER = ['claude', 'codex', 'kimi', 'opencode']

export function CreateAgentModal({ open, onOpenChange, onCreated }: Props) {
  const [config, setConfig] = useState<AgentConfigState>({
    name: '',
    display_name: '',
    description: '',
    runtime: 'claude',
    model: '',
    reasoningEffort: null,
    envVars: [],
  })
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const { runtimeStatuses, runtimeStatusError } = useRuntimeStatuses(open)

  // Reset form when modal closes.
  useEffect(() => {
    if (!open) {
      setConfig({ name: '', display_name: '', description: '', runtime: 'claude', model: '', reasoningEffort: null, envVars: [] })
      setError(null)
    }
  }, [open])

  // Default to the first installed ACP runtime once statuses load.
  useEffect(() => {
    if (runtimeStatuses.length === 0 || config.name !== '') return
    const acpRuntime = RUNTIME_ORDER.find((rt) =>
      runtimeStatuses.find((s) => s.runtime === rt && s.installed && s.driverMode === 'acp'),
    )
    if (acpRuntime && acpRuntime !== config.runtime) {
      setConfig((prev) => ({ ...prev, runtime: acpRuntime, model: '' }))
    }
  }, [runtimeStatuses]) // eslint-disable-line react-hooks/exhaustive-deps

  async function handleCreate() {
    if (!config.name.trim()) {
      setError('Name is required')
      return
    }
    setCreating(true)
    setError(null)
    try {
      if (!config.model.trim()) {
        throw new Error('Model is required')
      }
      const res = await fetch('/api/agents', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: config.name.trim(),
          display_name: config.display_name.trim(),
          description: config.description,
          runtime: config.runtime,
          model: config.model,
          reasoningEffort: config.runtime === 'codex' || config.runtime === 'opencode' ? config.reasoningEffort : null,
          envVars: config.envVars,
        }),
      })
      if (!res.ok) {
        const body = await res.json().catch(() => ({ error: res.statusText }))
        throw new Error((body as { error?: string }).error ?? res.statusText)
      }
      onCreated()
    } catch (e) {
      setError(String(e))
    } finally {
      setCreating(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(720px,96vw)]">
        <DialogHeader>
          <div className="flex flex-col gap-1">
            <DialogTitle>Create Agent</DialogTitle>
            <DialogDescription>[agent::new]</DialogDescription>
          </div>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>

        <FormField>
          <Label>Machine</Label>
          <Select value="local" disabled>
            <SelectTrigger aria-label="Machine">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="local">local</SelectItem>
            </SelectContent>
          </Select>
        </FormField>

        <AgentConfigForm
          state={config}
          runtimeStatuses={runtimeStatuses}
          runtimeStatusError={runtimeStatusError}
          editableName
          onChange={setConfig}
        />

        {error && <FormError>{error}</FormError>}

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button
            onClick={handleCreate}
            disabled={creating || !config.name.trim() || !config.model.trim()}
          >
            {creating ? 'Creating...' : 'Create Agent'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
