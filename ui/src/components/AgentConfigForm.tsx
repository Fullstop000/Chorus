import type { AgentEnvVar, RuntimeStatusInfo } from '../types'

export const MODELS: Record<string, { value: string; label: string }[]> = {
  claude: [
    { value: 'sonnet', label: 'claude-sonnet-4-6' },
    { value: 'opus', label: 'claude-opus-4-6' },
    { value: 'haiku', label: 'claude-haiku-4-5' },
  ],
  codex: [
    { value: 'gpt-5.4', label: 'gpt-5.4' },
    { value: 'gpt-5.4-mini', label: 'gpt-5.4-mini' },
    { value: 'gpt-5.3-codex', label: 'gpt-5.3-codex' },
    { value: 'gpt-5.2-codex', label: 'gpt-5.2-codex' },
    { value: 'gpt-5.2', label: 'gpt-5.2' },
    { value: 'gpt-5.1-codex-max', label: 'gpt-5.1-codex-max' },
    { value: 'gpt-5.1-codex-mini', label: 'gpt-5.1-codex-mini' },
  ],
  kimi: [
    { value: 'kimi-code/kimi-for-coding', label: 'kimi-for-coding' },
  ],
}

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
    runtime === 'claude' ? 'Claude Code' : runtime === 'codex' ? 'Codex CLI' : 'Kimi CLI'
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
            <div className="modal-field">
              <label>Name</label>
              <input
                value={state.name}
                onChange={(e) => onChange({ ...state, name: e.target.value })}
                placeholder="e.g. my-agent"
                autoFocus
              />
              <div className="modal-field-hint">Stable machine name used in channels and internal references.</div>
            </div>
          )}

          <div className="modal-field">
            <label>Display Name</label>
            <input
              value={state.display_name}
              onChange={(e) => onChange({ ...state, display_name: e.target.value })}
              placeholder={state.name || 'Agent name'}
              autoFocus={!editableName}
            />
            <div className="modal-field-hint">Human-facing label shown across the workspace.</div>
          </div>
        </div>

        <div className="modal-field">
          <label>Role</label>
          <textarea
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
            <label>Runtime</label>
            <select
              value={state.runtime}
              onChange={(e) => {
                const runtime = e.target.value
                const model = MODELS[runtime]?.[0]?.value ?? ''
                onChange({
                  ...state,
                  runtime,
                  model,
                  reasoningEffort: runtime === 'codex' ? state.reasoningEffort ?? 'default' : null,
                })
              }}
            >
              <option value="claude">{runtimeOptionLabel('claude', runtimeStatuses)}</option>
              <option value="codex">{runtimeOptionLabel('codex', runtimeStatuses)}</option>
              <option value="kimi">{runtimeOptionLabel('kimi', runtimeStatuses)}</option>
            </select>
            <div className={`runtime-status-banner runtime-status-banner-${runtimeSummary.tone}`}>
              <strong>{runtimeSummary.title}</strong>
              <span>{runtimeSummary.detail}</span>
            </div>
            {runtimeStatusError && (
              <div className="modal-field-hint">{runtimeStatusError}</div>
            )}
          </div>

          <div className="modal-field">
            <label>Model</label>
            <select
              value={state.model}
              onChange={(e) => onChange({ ...state, model: e.target.value })}
            >
              {(MODELS[state.runtime] ?? []).map((model) => (
                <option key={model.value} value={model.value}>
                  {model.label}
                </option>
              ))}
            </select>
          </div>

          {state.runtime === 'codex' && (
            <div className="modal-field">
              <label>Reasoning</label>
              <select
                value={state.reasoningEffort ?? 'default'}
                onChange={(e) =>
                  onChange({
                    ...state,
                    reasoningEffort: e.target.value,
                  })
                }
              >
                {REASONING_EFFORTS.map((effort) => (
                  <option key={effort.value} value={effort.value}>
                    {effort.label}
                  </option>
                ))}
              </select>
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
          <label>Environment Variables</label>
          <div className="modal-field-hint">Pass runtime secrets and flags into the agent process without hardcoding them into prompts.</div>
          <div className="env-var-editor">
            {state.envVars.length === 0 && (
              <div className="env-var-editor-empty">No environment variables configured.</div>
            )}
            {state.envVars.map((envVar, index) => (
              <div key={index} className="env-var-editor-row">
                <input
                  value={envVar.key}
                  onChange={(e) => updateEnvVar(index, 'key', e.target.value)}
                  placeholder="KEY"
                />
                <input
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
