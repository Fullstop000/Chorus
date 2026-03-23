import type { AgentEnvVar } from '../types'

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
  editableName?: boolean
  onChange: (next: AgentConfigState) => void
}

export function AgentConfigForm({ state, editableName = false, onChange }: Props) {
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

  return (
    <>
      {editableName && (
        <div className="modal-field">
          <label>Name</label>
          <input
            value={state.name}
            onChange={(e) => onChange({ ...state, name: e.target.value })}
            placeholder="e.g. my-agent"
            autoFocus
          />
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
      </div>

      <div className="modal-field">
        <label>Role</label>
        <textarea
          value={state.description}
          onChange={(e) => onChange({ ...state, description: e.target.value })}
          placeholder="What does this agent do?"
        />
      </div>

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
          <option value="claude">Claude Code</option>
          <option value="codex">Codex CLI</option>
        </select>
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

      <div className="modal-field">
        <label>Environment Variables</label>
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
          <button className="env-add-btn" type="button" onClick={addEnvVar}>
            + Add variable
          </button>
        </div>
      </div>
    </>
  )
}
