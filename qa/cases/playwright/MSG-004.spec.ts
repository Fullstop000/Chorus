import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser } from './helpers/api'
import { openAgentChat, openAgentTab, sendChatMessage , gotoApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/messaging.md` — MSG-004 Direct Message Wake And Reply Visibility
 */
test.describe('MSG-004', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Direct Message Wake And Reply Visibility @case MSG-004', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    const { username } = await getWhoami(request)
    const token = `dm-wake-${Date.now()}`
    await request.post('/api/agents/bot-a/stop')
    await gotoApp(page)

    await test.step('Steps 1–5: Send DM to inactive bot-a and wait for wake + reply', async () => {
      await openAgentChat(page, 'bot-a')
      await openAgentTab(page, 'bot-a', 'Profile')
      await expect(page.locator('.profile-config-grid')).toContainText('inactive')
      await page.getByRole('button', { name: 'Chat' }).click()
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

    await test.step('Steps 6–9: Reply stays in same DM and lifecycle surfaces recover coherently', async () => {
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
      await openAgentTab(page, 'bot-a', 'Profile')
      await expect(page.locator('.profile-config-grid')).toContainText('active')
      await page.getByRole('button', { name: 'Chat' }).click()
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
    })
  })
})
