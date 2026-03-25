import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio, recallApi, rememberApi } from './helpers/api'

/**
 * Catalog: `qa/cases/shared_memory.md` — MEM-002 Recall Returns Previously Stored Fact
 */
test.describe('MEM-002', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Recall Returns Previously Stored Fact @case MEM-002', async ({ request }) => {
    const token = `mem-recall-${Date.now()}`
    await rememberApi(request, 'bot-a', {
      key: 'test-finding',
      value: token,
      tags: ['qa'],
      channelContext: 'all',
    })
    const res = await recallApi(request, 'bot-b', { tags: 'qa' })
    expect(res.entries.some((entry) => entry.value === token)).toBe(true)
    expect(res.entries.some((entry) => entry.author_agent_id === 'bot-a')).toBe(true)
  })
})
