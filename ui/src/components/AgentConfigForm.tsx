import { useEffect } from 'react'
import type { AgentEnvVar, RuntimeStatusInfo } from '../types'
import { useRuntimeModels } from '../hooks/useRuntimeModels'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Label } from '@/components/ui/label'
import { FormField } from '@/components/ui/form'
import { Button } from '@/components/ui/button'

export const REASONING_EFFORTS = [
  { value: 'default', label: 'Default' },
  { value: 'none', label: 'None' },
  { value: 'minimal', label: 'Minimal' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High' },
  { value: 'xhigh', label: 'Extra High' },
]

export interface AgentConfigState {
  name: string
  display_name: string
  description: string
  runtime: string
  model: string
  reasoningEffort: string | null
  envVars: AgentEnvVar[]
}

interface Props {
  state: AgentConfigState
  runtimeStatuses?: RuntimeStatusInfo[]
  runtimeStatusError?: string | null
  editableName?: boolean
  onChange: (next: AgentConfigState) => void
}

export function runtimeOptionLabel(
  runtime: string,
  runtimeStatuses: RuntimeStatusInfo[] = [],
): string {
  const baseLabel =
    runtime === 'claude' ? 'Claude Code' : runtime === 'codex' ? 'Codex CLI' : runtime === 'opencode' ? 'OpenCode' : 'Kimi CLI'
  const status = runtimeStatuses.find((entry) => entry.runtime === runtime)
  if (!status) return `${baseLabel} · status unavailable`
  if (!status.installed) return `${baseLabel} · not installed`
  if (status.authStatus === 'authed') return `${baseLabel} · signed in`
  return `${baseLabel} · not signed in`
}

export function runtimeStatusSummary(
  runtime: string,
  runtimeStatuses: RuntimeStatusInfo[] = [],
): { tone: 'ok' | 'warn' | 'muted'; title: string; detail: string } {
  const status = runtimeStatuses.find((entry) => entry.runtime === runtime)
  if (!status) {
    return {
      tone: 'muted',
      title: 'Status unavailable',
      detail: 'The local runtime probe did not return a status for this CLI.',
    }
  }
  if (!status.installed) {
    return {
      tone: 'warn',
      title: 'Not installed',
      detail: 'This runtime is not available on the local machine yet.',
    }
  }
  if (status.authStatus === 'authed') {
    return {
      tone: 'ok',
      title: 'Signed in',
      detail: 'This runtime is installed locally and has an active login.',
    }
  }
  return {
    tone: 'warn',
    title: 'Not signed in',
    detail: 'The CLI is installed, but local authentication needs to be completed before agent startup will work reliably.',
  }
}

export function AgentConfigForm({
  state,
  runtimeStatuses = [],
  runtimeStatusError = null,
  editableName = false,
  onChange,
}: Props) {
  const { runtimeModels, runtimeModelsError } = useRuntimeModels(state.runtime)

  useEffect(() => {
    if (runtimeModels.length === 0 || runtimeModels.includes(state.model)) {
      return
    }

    onChange({
      ...state,
      model: runtimeModels[0],
    })
  }, [onChange, runtimeModels, state])

  function updateEnvVar(index: number, key: keyof AgentEnvVar, value: string) {
    const envVars = state.envVars.map((envVar, envIndex) =>
      envIndex === index ? { ...envVar, [key]: value } : envVar
    )
    onChange({ ...state, envVars })
  }

  function addEnvVar() {
    onChange({
      ...state,
      envVars: [...state.envVars, { key: '', value: '' }],
    })
  }

  function removeEnvVar(index: number) {
    onChange({
      ...state,
      envVars: state.envVars.filter((_, envIndex) => envIndex !== index),
    })
  }

  const runtimeSummary = runtimeStatusSummary(state.runtime, runtimeStatuses)

  return (
    <div className="agent-config-form">
      <section className="agent-config-section">
        <div className="agent-config-section-header">
          <span className="agent-config-section-kicker">[identity::surface]</span>
        </div>
        <div className="agent-config-grid">
          {editableName && (
            <FormField>
              <Label>Name</Label>
              <Input
                value={state.name}
                onChange={(e) => onChange({ ...state, name: e.target.value })}
                placeholder="e.g. my-agent"
                autoFocus
              />
              <p className="text-xs text-muted-foreground leading-relaxed mt-1">Stable machine name used in channels and internal references.</p>
            </FormField>
          )}

          <FormField>
            <Label>Display Name</Label>
            <Input
              value={state.display_name}
              onChange={(e) => onChange({ ...state, display_name: e.target.value })}
              placeholder={state.name || 'Agent name'}
              autoFocus={!editableName}
            />
            <p className="text-xs text-muted-foreground leading-relaxed mt-1">Human-facing label shown across the workspace.</p>
          </FormField>
        </div>

        <FormField>
          <Label>Role</Label>
          <Textarea
            value={state.description}
            onChange={(e) => onChange({ ...state, description: e.target.value })}
            placeholder="What does this agent do?"
          />
          <p className="text-xs text-muted-foreground leading-relaxed mt-1">Keep it brief and operational. This description guides how teammates interpret the agent.</p>
        </FormField>
      </section>

      <section className="agent-config-section">
        <div className="agent-config-section-header">
          <span className="agent-config-section-kicker">[runtime::selection]</span>
        </div>
        <div className="agent-config-grid">
          <FormField>
            <Label>Runtime</Label>
            <Select
              value={state.runtime}
              onValueChange={(runtime) => {
                onChange({
                  ...state,
                  runtime,
                  model: '',
                  reasoningEffort: runtime === 'codex' || runtime === 'opencode' ? state.reasoningEffort ?? 'default' : null,
                })
              }}
            >
              <SelectTrigger aria-label="Runtime">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="claude">{runtimeOptionLabel('claude', runtimeStatuses)}</SelectItem>
                <SelectItem value="codex">{runtimeOptionLabel('codex', runtimeStatuses)}</SelectItem>
                <SelectItem value="kimi">{runtimeOptionLabel('kimi', runtimeStatuses)}</SelectItem>
                <SelectItem value="opencode">{runtimeOptionLabel('opencode', runtimeStatuses)}</SelectItem>
              </SelectContent>
            </Select>
            <div className={`runtime-status-banner runtime-status-banner-${runtimeSummary.tone}`}>
              <strong>{runtimeSummary.title}</strong>
              <span>{runtimeSummary.detail}</span>
            </div>
            {runtimeStatusError && (
              <p className="text-xs text-muted-foreground leading-relaxed mt-1">{runtimeStatusError}</p>
            )}
          </FormField>

          <FormField>
            <Label>Model</Label>
            <Select
              value={state.model}
              onValueChange={(model) => onChange({ ...state, model })}
            >
              <SelectTrigger aria-label="Model">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {runtimeModels.map((model) => (
                  <SelectItem key={model} value={model}>
                    {model}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {runtimeModelsError && (
              <p className="text-xs text-muted-foreground leading-relaxed mt-1">{runtimeModelsError}</p>
            )}
          </FormField>

          {(state.runtime === 'codex' || state.runtime === 'opencode') && (
            <FormField>
              <Label>Reasoning</Label>
              <Select
                value={state.reasoningEffort ?? 'default'}
                onValueChange={(reasoningEffort) =>
                  onChange({
                    ...state,
                    reasoningEffort,
                  })
                }
              >
                <SelectTrigger aria-label="Reasoning">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {REASONING_EFFORTS.map((effort) => (
                    <SelectItem key={effort.value} value={effort.value}>
                      {effort.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </FormField>
          )}
        </div>
      </section>

      <section className="agent-config-section">
        <div className="agent-config-section-header">
          <span className="agent-config-section-kicker">[env::bindings]</span>
          <Button size="sm" variant="ghost" type="button" onClick={addEnvVar}>
            + Add variable
          </Button>
        </div>
        <FormField>
          <Label>Environment Variables</Label>
          <p className="text-xs text-muted-foreground leading-relaxed mt-1">Pass runtime secrets and flags into the agent process without hardcoding them into prompts.</p>
          <div className="env-var-editor">
            {state.envVars.length === 0 && (
              <div className="env-var-editor-empty">No environment variables configured.</div>
            )}
            {state.envVars.map((envVar, index) => (
              <div key={index} className="env-var-editor-row">
                <Input
                  value={envVar.key}
                  onChange={(e) => updateEnvVar(index, 'key', e.target.value)}
                  placeholder="KEY"
                />
                <Input
                  value={envVar.value}
                  onChange={(e) => updateEnvVar(index, 'value', e.target.value)}
                  placeholder="value"
                />
                <Button size="sm" variant="ghost" type="button" onClick={() => removeEnvVar(index)}>
                  ×
                </Button>
              </div>
            ))}
          </div>
        </FormField>
      </section>
    </div>
  )
}
