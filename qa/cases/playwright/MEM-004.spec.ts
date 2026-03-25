import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser, rememberApi, recallApi, sendAsUser } from './helpers/api'
import { clickSidebarChannel } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/shared_memory.md` — MEM-004 Two-Agent Handoff Via Shared Memory
 */
test.describe('MEM-004', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Two-Agent Handoff Via Shared Memory @case MEM-004', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    const { username } = await getWhoami(request)
    const token = `handoff-${Date.now()}`
    await rememberApi(request, 'bot-a', {
      key: 'handoff-test',
      value: token,
      tags: ['handoff-test'],
      channelContext: 'all',
    })
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 3–7: Breadcrumb visible and second agent can use recall-backed handoff', async () => {
      await clickSidebarChannel(page, 'shared-memory')
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
      const recall = await recallApi(request, 'bot-b', { tags: 'handoff-test' })
      expect(recall.entries.some((entry) => entry.value === token)).toBe(true)
      await sendAsUser(
        request,
        username,
        '#all',
        `@bot-b use shared memory tag handoff-test and reply with token ${token}`
      )
      const deadline = Date.now() + 120_000
      let sawReply = false
      while (Date.now() < deadline) {
        const history = await historyForUser(request, username, '#all', 50)
        sawReply = history.some((m) => m.senderName === 'bot-b' && (m.content ?? '').includes(token))
        if (sawReply) break
        await new Promise((r) => setTimeout(r, 4000))
      }
      expect(sawReply).toBe(true)
    })
  })
})
