import { useState, useEffect } from 'react'
import { ArrowLeft } from 'lucide-react'
import { useRuntimeStatuses } from '../../hooks/useRuntimeStatuses'
import { useTemplates } from '../../hooks/useTemplates'
import { AgentConfigForm, type AgentConfigState } from './AgentConfigForm'
import { TemplateGallery, TemplatePreview } from './TemplateGallery'
import { LaunchTrio } from './LaunchTrio'
import { Dialog, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { FormError } from '@/components/ui/form'
import type { AgentTemplate } from '../../hooks/useTemplates'
import './CreateAgentModal.css'

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

const RUNTIME_ORDER = ['claude', 'codex', 'kimi', 'opencode']

type Step = 'browse' | 'configure'

export function CreateAgentModal({ open, onOpenChange, onCreated }: Props) {
  const [step, setStep] = useState<Step>('browse')
  const [config, setConfig] = useState<AgentConfigState>({ ...EMPTY_CONFIG })
  const [selectedTemplate, setSelectedTemplate] = useState<AgentTemplate | null>(null)
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const { runtimeStatuses, runtimeStatusError } = useRuntimeStatuses(open)
  const { categories, allTemplates, isLoading: templatesLoading } = useTemplates(open)

  const hasTemplates = !templatesLoading && allTemplates.length > 0

  function handleTemplateSelect(template: AgentTemplate | null) {
    if (!template) return
    setSelectedTemplate(template)
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
    setStep('configure')
  }

  function handleFromScratch() {
    setSelectedTemplate(null)
    setConfig({ ...EMPTY_CONFIG })
    setStep('configure')
  }

  function handleBack() {
    setSelectedTemplate(null)
    setConfig({ ...EMPTY_CONFIG })
    setStep('browse')
  }

  function handleTrioLaunched(_channelId: string) {
    onCreated()
  }

  // Reset everything when modal closes.
  useEffect(() => {
    if (!open) {
      setStep('browse')
      setConfig({ ...EMPTY_CONFIG })
      setSelectedTemplate(null)
      setError(null)
    }
  }, [open])

  // If no templates, skip straight to configure step.
  useEffect(() => {
    if (open && !templatesLoading && !hasTemplates) {
      setStep('configure')
    }
  }, [open, templatesLoading, hasTemplates])

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

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(720px,96vw)]">
        <DialogHeader>
          <div className="flex flex-col gap-1">
            <DialogTitle>
              {step === 'browse' ? 'Create Agent' : (
                <span className="modal-title-with-back">
                  <button
                    onClick={handleBack}
                    className="modal-back-btn"
                    aria-label="Back to templates"
                  >
                    <ArrowLeft size={16} />
                  </button>
                  Configure Agent
                </span>
              )}
            </DialogTitle>
            <DialogDescription>
              {step === 'browse' ? '[agent::new]' : (selectedTemplate ? selectedTemplate.name : '[from scratch]')}
            </DialogDescription>
          </div>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>

        {/* Step 1: Browse templates */}
        {step === 'browse' && (
          <>
            <LaunchTrio
              allTemplates={allTemplates}
              onLaunched={handleTrioLaunched}
            />

            <div className="system-message-divider">
              <div className="system-message-divider__line" />
              <span className="system-message-divider__label">or choose a template</span>
              <div className="system-message-divider__line" />
            </div>

            <TemplateGallery
              categories={categories}
              allTemplates={allTemplates}
              onSelect={handleTemplateSelect}
            />

            <DialogFooter>
              <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
              <Button variant="outline" onClick={handleFromScratch}>
                Create from scratch
              </Button>
            </DialogFooter>
          </>
        )}

        {/* Step 2: Configure agent */}
        {step === 'configure' && (
          <>
            {selectedTemplate && <TemplatePreview template={selectedTemplate} />}

            <AgentConfigForm
              state={config}
              runtimeStatuses={runtimeStatuses}
              runtimeStatusError={runtimeStatusError}
              editableName
              onChange={setConfig}
            />

            {error && <FormError>{error}</FormError>}

            <DialogFooter>
              {hasTemplates && (
                <Button variant="outline" onClick={handleBack}>Back</Button>
              )}
              <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
              <Button
                onClick={handleCreate}
                disabled={creating || !config.name.trim() || !config.model.trim()}
              >
                {creating ? 'Creating...' : 'Create Agent'}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}
