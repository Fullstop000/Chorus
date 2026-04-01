import { useEffect } from 'react'
import { LoaderCircle } from 'lucide-react'
import type { AgentEnvVar, RuntimeStatusInfo } from '../types'
import { useRuntimeModels } from '../hooks/useRuntimeModels'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

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

export function modelSelectDisplayLabel({
  selectedModel,
  runtimeModels,
  isLoading,
}: {
  selectedModel: string
  runtimeModels: string[]
  isLoading: boolean
}): string {
  if (isLoading) return 'Loading models...'
  if (selectedModel) return selectedModel
  if (runtimeModels.length > 0) return runtimeModels[0]
  return 'No models available'
}

export function AgentConfigForm({
  state,
  runtimeStatuses = [],
  runtimeStatusError = null,
  editableName = false,
  onChange,
}: Props) {
  const { runtimeModels, runtimeModelsError, isLoading } = useRuntimeModels(state.runtime)

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
  const modelLabel = modelSelectDisplayLabel({
    selectedModel: state.model,
    runtimeModels,
    isLoading,
  })

  return (
    <div className="agent-config-form">
      <section className="agent-config-section">
        <div className="agent-config-section-header">
          <span className="agent-config-section-kicker">[identity::surface]</span>
        </div>
        <div className="agent-config-grid">
          {editableName && (
            <div className="modal-field">
              <label className="form-label">Name</label>
              <input
                className="form-input"
                value={state.name}
                onChange={(e) => onChange({ ...state, name: e.target.value })}
                placeholder="e.g. my-agent"
                autoFocus
              />
              <div className="modal-field-hint">Stable machine name used in channels and internal references.</div>
            </div>
          )}

          <div className="modal-field">
            <label className="form-label">Display Name</label>
            <input
              className="form-input"
              value={state.display_name}
              onChange={(e) => onChange({ ...state, display_name: e.target.value })}
              placeholder={state.name || 'Agent name'}
              autoFocus={!editableName}
            />
            <div className="modal-field-hint">Human-facing label shown across the workspace.</div>
          </div>
        </div>

        <div className="modal-field">
          <label className="form-label">Role</label>
          <textarea
            className="form-textarea"
            value={state.description}
            onChange={(e) => onChange({ ...state, description: e.target.value })}
            placeholder="What does this agent do?"
          />
          <div className="modal-field-hint">Keep it brief and operational. This description guides how teammates interpret the agent.</div>
        </div>
      </section>

      <section className="agent-config-section">
        <div className="agent-config-section-header">
          <span className="agent-config-section-kicker">[runtime::selection]</span>
        </div>
        <div className="agent-config-grid">
          <div className="modal-field">
            <label className="form-label">Runtime</label>
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
              <SelectTrigger className="form-select" aria-label="Runtime">
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
              <div className="modal-field-hint">{runtimeStatusError}</div>
            )}
          </div>

          <div className="modal-field">
            <label className="form-label">Model</label>
            <Select
              value={state.model}
              onValueChange={(model) => onChange({ ...state, model })}
              disabled={isLoading || runtimeModels.length === 0}
            >
              <SelectTrigger className="form-select" aria-label="Model">
                {isLoading ? (
                  <span className="select-trigger-loading">
                    <LoaderCircle size={14} className="select-trigger-spinner" />
                    <span>{modelLabel}</span>
                  </span>
                ) : (
                  <span className="select-trigger-text">{modelLabel}</span>
                )}
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
              <div className="modal-field-hint">{runtimeModelsError}</div>
            )}
          </div>

          {(state.runtime === 'codex' || state.runtime === 'opencode') && (
            <div className="modal-field">
              <label className="form-label">Reasoning</label>
              <Select
                value={state.reasoningEffort ?? 'default'}
                onValueChange={(reasoningEffort) =>
                  onChange({
                    ...state,
                    reasoningEffort,
                  })
                }
              >
                <SelectTrigger className="form-select" aria-label="Reasoning">
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
            </div>
          )}
        </div>
      </section>

      <section className="agent-config-section">
        <div className="agent-config-section-header">
          <span className="agent-config-section-kicker">[env::bindings]</span>
          <button className="env-add-btn" type="button" onClick={addEnvVar}>
            + Add variable
          </button>
        </div>
        <div className="modal-field">
          <label className="form-label">Environment Variables</label>
          <div className="modal-field-hint">Pass runtime secrets and flags into the agent process without hardcoding them into prompts.</div>
          <div className="env-var-editor">
            {state.envVars.length === 0 && (
              <div className="env-var-editor-empty">No environment variables configured.</div>
            )}
            {state.envVars.map((envVar, index) => (
              <div key={index} className="env-var-editor-row">
                <input
                  className="form-input"
                  value={envVar.key}
                  onChange={(e) => updateEnvVar(index, 'key', e.target.value)}
                  placeholder="KEY"
                />
                <input
                  className="form-input"
                  value={envVar.value}
                  onChange={(e) => updateEnvVar(index, 'value', e.target.value)}
                  placeholder="value"
                />
                <button className="env-remove-btn" type="button" onClick={() => removeEnvVar(index)}>
                  ×
                </button>
              </div>
            ))}
          </div>
        </div>
      </section>
    </div>
  )
}
