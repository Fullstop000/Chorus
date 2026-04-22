import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser, stopAgentApi, type TrioNames } from './helpers/api'
import { expectProfileStatus, gotoApp, openAgentChat, openAgentTab, sendChatMessage } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/messaging.md` — MSG-004 Direct Message Wake And Reply Visibility
 *
 * Uses bot-b (kimi) because it reliably responds when woken.
 */
let trio: TrioNames

test.describe('MSG-004', () => {
  test.beforeAll(async ({ request }) => {
    trio = await ensureMixedRuntimeTrio(request)
  })

  test('Direct Message Wake And Reply Visibility @case MSG-004', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(120_000)
    const { username } = await getWhoami(request)
    const token = `dm-wake-${Date.now()}`
    const dmChannel = `dm:@${trio.botB}`
    await stopAgentApi(request, trio.botB)
    await gotoApp(page)

    await test.step('Steps 1–5: Send DM to asleep bot-b and wait for wake + reply', async () => {
      await openAgentChat(page, trio.displayB)
      await openAgentTab(page, trio.displayB, 'Profile')
      await expectProfileStatus(page, 'asleep')
      await page.getByRole('button', { name: 'Chat', exact: true }).click()
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

    await test.step('Steps 6–9: Reply stays in same DM and lifecycle surfaces recover coherently', async () => {
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
      await openAgentTab(page, trio.displayB, 'Profile')
      await expectProfileStatus(page, ['ready', 'working'])
      await page.getByRole('button', { name: 'Chat', exact: true }).click()
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
    })
  })
})
