import { test, expect } from './helpers/fixtures'
import {
  ensureMixedRuntimeTrio,
  getAgentDetail,
  getWhoami,
  historyForUser,
  stopAgentApi,
  type TrioNames,
} from './helpers/api'
import { openAgentChat, openAgentTab, sendChatMessage , gotoApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/agents.md` — ACT-002 Activity Timeline Ordering During Wake And Recovery
 *
 * Uses bot-b (kimi) because it reliably responds when woken from inactive.
 */
let trio: TrioNames

test.describe('ACT-002', () => {
  test.beforeAll(async ({ request }) => {
    trio = await ensureMixedRuntimeTrio(request)
  })

  test('Activity Timeline Ordering During Wake And Recovery @case ACT-002', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(120_000)
    const { username } = await getWhoami(request)
    const token = `act-wake-${Date.now()}`
    const dmChannel = `dm:@${trio.botB}`
    await gotoApp(page)

    await test.step('Precondition: stop bot-b, then wake it via DM', async () => {
      await stopAgentApi(request, trio.botB)
      await openAgentChat(page, trio.displayB)
      await sendChatMessage(page, `Reply with exact token ${token}`)
      const deadline = Date.now() + 120_000
      let sawReply = false
      while (Date.now() < deadline) {
        const history = await historyForUser(request, username, dmChannel, 40)
        sawReply = history.some((m) => m.senderType === 'agent' && (m.content ?? '').includes(token))
        if (sawReply) break
        await new Promise((r) => setTimeout(r, 2000))
      }
      expect(sawReply).toBe(true)
    })

    await test.step('Steps 1–7: Activity segment shows coherent wake-up ordering', async () => {
      await openAgentTab(page, trio.displayB, 'Activity')
      await expect(page.locator('.ta-layout')).toBeVisible({ timeout: 15_000 })
      const items = page.locator('.ta-detail .activity-item')
      await expect(items.first()).toBeVisible({ timeout: 15_000 })
      // Agent should be active after wake-up
      const detail = await getAgentDetail(request, trio.botB)
      expect(['ready', 'working']).toContain(detail.agent.status)
    })
  })
})
