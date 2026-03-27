import { describe, expect, it } from 'vitest'
import { runtimeOptionLabel, runtimeStatusSummary } from './AgentConfigForm'

describe('runtimeOptionLabel', () => {
  it('shows not installed copy when the runtime is missing', () => {
    expect(
      runtimeOptionLabel('kimi', [
        { runtime: 'kimi', installed: false },
      ])
    ).toContain('not installed')
  })

  it('shows signed in copy for authenticated runtimes', () => {
    expect(
      runtimeOptionLabel('claude', [
        { runtime: 'claude', installed: true, authStatus: 'authed' },
      ])
    ).toContain('signed in')
  })
})

describe('runtimeStatusSummary', () => {
  it('warns when a runtime is installed but not signed in', () => {
    expect(
      runtimeStatusSummary('codex', [
        { runtime: 'codex', installed: true, authStatus: 'unauthed' },
      ])
    ).toEqual({
      tone: 'warn',
      title: 'Not signed in',
      detail:
        'The CLI is installed, but local authentication needs to be completed before agent startup will work reliably.',
    })
  })
})
