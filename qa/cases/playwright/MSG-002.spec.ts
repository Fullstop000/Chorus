import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser, type TrioNames } from './helpers/api'
import { openAgentChat, openThreadFromMessage, sendChatMessage , gotoApp , reloadApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/messaging.md` — MSG-002 Direct Message Round-Trip
 * Supersedes: DM-002
 *
 * Uses bot-b (kimi) as the DM target because it reliably responds.
 */
let trio: TrioNames

test.describe('MSG-002', () => {
  test.beforeAll(async ({ request }) => {
    trio = await ensureMixedRuntimeTrio(request)
  })

  test('Direct Message Round-Trip @case MSG-002', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(180_000)

    const { username } = await getWhoami(request)
    const token = `dm-check-${Date.now()}`
    const prompt = `Reply in this DM, not in a thread. Return exact token: ${token}`
    let replyMode: 'top-level' | 'thread' = 'top-level'
    const dmChannel = `dm:@${trio.botB}`

    await gotoApp(page)

    await test.step('Step 1: Open DM with bot-b', async () => {
      await openAgentChat(page, trio.displayB)
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
        const msgs = await historyForUser(request, username, dmChannel, 40)
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
          const threadMsgs = await historyForUser(request, username, `${dmChannel}:${parent.id}`, 40)
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
      await openAgentChat(page, trio.displayB)
      if (replyMode === 'top-level') {
        await expect(page.getByText(token).first()).toBeVisible({ timeout: 15_000 })
      } else {
        await openThreadFromMessage(page, token)
        await expect(page.locator('.thread-body')).toContainText(token, { timeout: 15_000 })
      }
    })

    await test.step('Step 9: Switch target and return to DM', async () => {
      await page.locator('.sidebar-item-text:text("all")').first().click()
      await openAgentChat(page, trio.displayB)
      if (replyMode === 'top-level') {
        await expect(page.getByText(token).first()).toBeVisible({ timeout: 15_000 })
      } else {
        await openThreadFromMessage(page, token)
        await expect(page.locator('.thread-body')).toContainText(token, { timeout: 15_000 })
      }
    })
  })
})
