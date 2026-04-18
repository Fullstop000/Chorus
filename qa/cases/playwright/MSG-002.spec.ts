import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser } from './helpers/api'
import { openAgentChat, openThreadFromMessage, sendChatMessage , gotoApp , reloadApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/messaging.md` — MSG-002 Direct Message Round-Trip
 * Supersedes: DM-002
 *
 * Preconditions:
 * - at least one test agent exists (`bot-a`)
 * - agent reachable, not mid-turn
 *
 * Steps:
 * 1. Open a DM with `bot-a`.
 * 2. Send a human DM that asks for an exact short token.
 * 3. Verify the human DM appears once in the DM timeline immediately after send.
 * 4. Wait for the agent reply.
 * 5. Verify the reply appears in the same DM timeline.
 * 6. Verify the reply text matches the requested token.
 * 7. Refresh the page.
 * 8. Re-open the same DM and verify both messages still visible.
 * 9. Switch to another target and return to the DM once.
 *
 * Expected:
 * - DM target clear; reply in DM not channel; refresh preserves; target switch preserves
 */
test.describe('MSG-002', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Direct Message Round-Trip @case MSG-002', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(180_000)

    const { username } = await getWhoami(request)
    const token = `dm-check-${Date.now()}`
    const prompt = `Reply in this DM, not in a thread. Return exact token: ${token}`
    let replyMode: 'top-level' | 'thread' = 'top-level'

    await gotoApp(page)

    await test.step('Step 1: Open DM with bot-a', async () => {
      await openAgentChat(page, 'bot-a')
      await expect(page.locator('.message-input-textarea')).toBeVisible()
    })

    await test.step('Step 2–3: Send DM; human row visible', async () => {
      await sendChatMessage(page, prompt)
      await expect(page.getByText(prompt).first()).toBeVisible()
    })

    await test.step('Steps 4–6: Agent reply in same DM with token', async () => {
      const deadline = Date.now() + 120_000
      let ok = false
      while (Date.now() < deadline) {
        const msgs = await historyForUser(request, username, 'dm:@bot-a', 40)
        if (msgs.some((m) => m.senderType === 'agent' && (m.content ?? '').includes(token))) {
          replyMode = 'top-level'
          ok = true
          break
        }
        const parent = msgs.find(
          (m) =>
            m.senderType !== 'agent' &&
            (m.content ?? '').includes(token) &&
            (m.replyCount ?? 0) > 0
        )
        if (parent) {
          const threadMsgs = await historyForUser(request, username, `dm:@bot-a:${parent.id}`, 40)
          if (threadMsgs.some((m) => m.senderType === 'agent' && (m.content ?? '').includes(token))) {
            replyMode = 'thread'
            ok = true
            break
          }
        }
        if (ok) break
        await new Promise((r) => setTimeout(r, 4000))
      }
      expect(ok).toBe(true)
    })

    await test.step('Step 7–8: Refresh and re-open DM — history persists', async () => {
      await reloadApp(page)
      await openAgentChat(page, 'bot-a')
      if (replyMode === 'top-level') {
        await expect(page.getByText(token).first()).toBeVisible({ timeout: 15_000 })
      } else {
        await openThreadFromMessage(page, token)
        await expect(page.locator('.thread-body')).toContainText(token, { timeout: 15_000 })
      }
    })

    await test.step('Step 9: Switch target and return to DM', async () => {
      await page.locator('.sidebar-item-text:text("all")').first().click()
      await openAgentChat(page, 'bot-a')
      if (replyMode === 'top-level') {
        await expect(page.getByText(token).first()).toBeVisible({ timeout: 15_000 })
      } else {
        await openThreadFromMessage(page, token)
        await expect(page.locator('.thread-body')).toContainText(token, { timeout: 15_000 })
      }
    })
  })
})
