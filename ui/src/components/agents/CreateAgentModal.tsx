import { useState } from 'react'
import { useRuntimeStatuses } from '../../hooks/useRuntimeStatuses'
import { useTemplates } from '../../hooks/useTemplates'
import { AgentConfigForm, type AgentConfigState } from './AgentConfigForm'
import { TemplateGallery, TemplatePreview } from './TemplateGallery'
import { LaunchTrio } from './LaunchTrio'
import { Dialog, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { FormField, FormError } from '@/components/ui/form'
import { Label } from '@/components/ui/label'
import type { AgentTemplate } from '../../hooks/useTemplates'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  onCreated: () => void
}

const EMPTY_CONFIG: AgentConfigState = {
  name: '',
  display_name: '',
  description: '',
  systemPrompt: null,
  runtime: 'claude',
  model: '',
  reasoningEffort: null,
  envVars: [],
}

export function CreateAgentModal({ open, onOpenChange, onCreated }: Props) {
  const [config, setConfig] = useState<AgentConfigState>({ ...EMPTY_CONFIG })
  const [selectedTemplate, setSelectedTemplate] = useState<AgentTemplate | null>(null)
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const { runtimeStatuses, runtimeStatusError } = useRuntimeStatuses(open)
  const { categories, allTemplates, isLoading: templatesLoading } = useTemplates()

  function handleTemplateSelect(template: AgentTemplate | null) {
    setSelectedTemplate(template)
    if (template) {
      const agentName = template.id.split('/')[1] ?? template.id
      setConfig({
        name: agentName,
        display_name: template.name,
        description: template.description ?? '',
        systemPrompt: template.prompt_body,
        runtime: template.suggested_runtime,
        model: '',
        reasoningEffort: null,
        envVars: [],
      })
    } else {
      setConfig({ ...EMPTY_CONFIG })
    }
  }

  function handleTrioLaunched(_channelId: string) {
    onCreated()
  }

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
          systemPrompt: config.systemPrompt || null,
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

  const hasTemplates = !templatesLoading && allTemplates.length > 0

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

        {hasTemplates && (
          <>
            <TemplateGallery
              categories={categories}
              allTemplates={allTemplates}
              selected={selectedTemplate}
              onSelect={handleTemplateSelect}
            />

            {selectedTemplate && <TemplatePreview template={selectedTemplate} />}

            {!selectedTemplate && (
              <LaunchTrio
                allTemplates={allTemplates}
                onLaunched={handleTrioLaunched}
              />
            )}

            <div className="text-center text-xs text-muted-foreground py-1">
              — or create from scratch —
            </div>
          </>
        )}

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
