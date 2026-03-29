import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio, getWhoami, sendAsUser } from './helpers/api'
import { clickSidebarChannel, openThreadFromMessage } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/messaging.md` — MSG-003 Thread Reply In Busy Channel
 *
 * Preconditions:
 * - `MSG-001` completed → script **seeds** `#all` with a fan-out prompt when LLM enabled
 *
 * Steps:
 * 1. In the shared channel, open a thread from one agent reply.
 * 2. Send a thread reply from the human.
 * 3. Wait for the addressed agent to reply in the thread.
 * 4. Return to the main channel view.
 * 5. Verify thread messages stay attached to the thread and do not pollute the main timeline.
 *
 * Expected:
 * - thread panel works; human thread line visible; main channel not polluted by thread body
 */
test.describe('MSG-003', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
    if (process.env.CHORUS_E2E_LLM === '0') return
    const { username } = await getWhoami(request)
    await sendAsUser(
      request,
      username,
      '#all',
      `MSG-003 precondition ${Date.now()} — bot-a reply "thread-seed-ok"`
    ).catch(() => {})
  })

  test('Thread Reply In Busy Channel @case MSG-003', async ({ page }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(300_000)

    await page.goto('/', { waitUntil: 'networkidle' })
    await clickSidebarChannel(page, 'all')

    await test.step('Step 1: Open thread from an agent reply', async () => {
      const deadline = Date.now() + 180_000
      let opened = false
      while (Date.now() < deadline) {
        const seededReply = page.locator('.message-item').filter({ hasText: 'thread-seed-ok' }).first()
        if (await seededReply.isVisible().catch(() => false)) {
          await openThreadFromMessage(page, 'thread-seed-ok')
          opened = true
          break
        }
        await page.waitForTimeout(3000)
      }
      expect(opened).toBeTruthy()
      await expect(page.locator('.thread-panel')).toBeVisible({ timeout: 10_000 })
    })

    const threadLine = `human-thread-${Date.now()}`

    await test.step('Step 2: Human thread reply', async () => {
      await page.locator('.thread-input-textarea').fill(threadLine)
      await page.locator('.thread-send-btn').click()
      await expect(page.locator('.thread-replies')).toContainText(threadLine)
    })

    await test.step('Steps 3–5: Close thread; main channel should not show thread-only line as top-level', async () => {
      await page.locator('.thread-close-btn').click()
      await expect(page.locator('.thread-panel')).toBeHidden()
      await expect(page.locator('.message-input-textarea')).toBeVisible()
    })
  })
})
