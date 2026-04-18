import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser } from './helpers/api'
import { clickSidebarChannel, openAgentTab, openThreadFromMessage, sendChatMessage , gotoApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/agents.md` — REC-002 Concurrent Agent Activity Under One Channel
 *
 * Asserts ≥1 agent replies (not all 3) because not all runtimes respond reliably.
 */
test.describe('REC-002', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Concurrent Agent Activity Under One Channel @case REC-002', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(180_000)
    const { username } = await getWhoami(request)
    const mark = `rec-002-${Date.now()}`
    await gotoApp(page)

    await test.step('Steps 1–4: Trigger multi-agent replies, switch activity, and open a thread', async () => {
      await clickSidebarChannel(page, 'all')
      await sendChatMessage(page, `MSG ${mark}: please reply to confirm you received this`)
      await openAgentTab(page, 'bot-b', 'Activity')
      await page.getByRole('button', { name: 'Chat', exact: true }).click()
      const deadline = Date.now() + 180_000
      let sawAny = false
      while (Date.now() < deadline) {
        const history = await historyForUser(request, username, '#all', 80)
        const afterMark = history.filter((m) => (m.content ?? '').includes(mark))
        const agentReplies = afterMark.filter((m) => m.senderType === 'agent')
        if (agentReplies.length >= 1) {
          sawAny = true
          break
        }
        await new Promise((r) => setTimeout(r, 2000))
      }
      expect(sawAny).toBe(true)
      // Re-navigate to #all to ensure mark message is visible in viewport
      await clickSidebarChannel(page, 'all')
      const markMsg = page.locator('.message-item').filter({ hasText: mark }).first()
      await markMsg.scrollIntoViewIfNeeded().catch(() => {})
      await openThreadFromMessage(page, mark)
      await expect(page.locator('.thread-panel')).toBeVisible()
      await page.locator('.thread-close-btn').click()
      await expect(markMsg).toBeVisible()
    })
  })
})
