import { describe, expect, it } from 'vitest'
import { isRuntimeAvailable, modelSelectDisplayLabel, runtimeOptionLabel, runtimeStatusSummary } from './AgentConfigForm'

describe('runtimeOptionLabel', () => {
  it('shows not installed copy when the runtime is missing', () => {
    expect(
      runtimeOptionLabel('kimi', [
        { runtime: 'kimi', auth: 'not_installed' as const },
      ])
    ).toContain('not installed')
  })

  it('shows signed in copy for authenticated runtimes', () => {
    expect(
      runtimeOptionLabel('claude', [
        { runtime: 'claude', auth: 'authed' as const },
      ])
    ).toContain('signed in')
  })
})

describe('isRuntimeAvailable', () => {
  it('returns false when the runtime status is unavailable', () => {
    expect(isRuntimeAvailable('claude', [])).toBe(false)
  })

  it('returns false for runtimes that are not installed', () => {
    expect(
      isRuntimeAvailable('kimi', [
        { runtime: 'kimi', auth: 'not_installed' as const },
      ])
    ).toBe(false)
  })

  it('returns true for installed runtimes', () => {
    expect(
      isRuntimeAvailable('codex', [
        { runtime: 'codex', auth: 'unauthed' as const },
      ])
    ).toBe(true)
  })
})

describe('runtimeStatusSummary', () => {
  it('warns when a runtime is installed but not signed in', () => {
    expect(
      runtimeStatusSummary('codex', [
        { runtime: 'codex', auth: 'unauthed' as const },
      ])
    ).toEqual({
      tone: 'warn',
      title: 'Not signed in',
      detail:
        'The CLI is installed, but local authentication needs to be completed before agent startup will work reliably.',
    })
  })
})

describe('modelSelectDisplayLabel', () => {
  it('shows loading copy while models are being fetched', () => {
    expect(modelSelectDisplayLabel({
      selectedModel: 'openai/gpt-5.4',
      runtimeModels: [],
      isLoading: true,
    })).toBe('Loading models…')
  })

  it('falls back to the first available model when the selection is empty', () => {
    expect(modelSelectDisplayLabel({
      selectedModel: '',
      runtimeModels: ['openai/codex-mini-latest'],
      isLoading: false,
    })).toBe('openai/codex-mini-latest')
  })
})
