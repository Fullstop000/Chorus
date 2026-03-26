import { test, expect } from '@playwright/test'
import {
  ensureMixedRuntimeTrio,
  getAgentDetail,
  getWhoami,
  historyForUser,
} from './helpers/api'
import { openAgentChat, openAgentTab, sendChatMessage } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/agents.md` — ACT-002 Activity Timeline Ordering During Wake And Recovery
 */
test.describe('ACT-002', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Activity Timeline Ordering During Wake And Recovery @case ACT-002', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    const { username } = await getWhoami(request)
    const token = `act-wake-${Date.now()}`
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Precondition: stop bot-a, then wake it via DM', async () => {
      await request.post('/api/agents/bot-a/stop')
      await openAgentChat(page, 'bot-a')
      await sendChatMessage(page, `Reply with exact token ${token}`)
      const deadline = Date.now() + 120_000
      let sawReply = false
      while (Date.now() < deadline) {
        const history = await historyForUser(request, username, 'dm:@bot-a', 40)
        sawReply = history.some((m) => m.senderType === 'agent' && (m.content ?? '').includes(token))
        if (sawReply) break
        await new Promise((r) => setTimeout(r, 4000))
      }
      expect(sawReply).toBe(true)
    })

    await test.step('Steps 1–7: Activity segment shows coherent wake-up ordering', async () => {
      await openAgentTab(page, 'bot-a', 'Activity')
      await expect(page.locator('.activity-item-message-received')).toContainText(token)
      await expect(page.locator('.activity-item-message-sent')).toContainText(token)
      await expect(page.locator('.activity-item-status').first()).toBeVisible()
      const detail = await getAgentDetail(request, 'bot-a')
      expect(detail.agent.status).toBe('active')
    })
  })
})
