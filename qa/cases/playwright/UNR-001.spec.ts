import { test, expect } from './helpers/fixtures'
import { ensureMixedRuntimeTrio, getWhoami } from './helpers/api'
import { clickSidebarChannel, sendChatMessage, gotoApp } from './helpers/ui'

const skipLLM = process.env.CHORUS_E2E_LLM === '0'

/**
 * Catalog: `qa/cases/messaging.md` — UNR-001 Unread Badge Clears on Scroll-to-Bottom
 *
 * Preconditions:
 * - `bot-a` exists as an agent that can reply
 * - active test channel `#all` exists
 *
 * Steps:
 * 1. Navigate to app, open #all channel
 * 2. Send enough messages to create scrollable content
 * 3. Scroll to top so new messages arrive while user is not at bottom
 * 4. Send a prompt that triggers bot-a reply → creates unread state
 * 5. Verify NewMessageBadge appears with count > 0
 * 6. Click the "N new messages" badge to jump to bottom
 * 7. Verify badge and divider disappear immediately (count = 0)
 *
 * Expected:
 * - Badge visible before scroll-to-bottom, hidden after
 * - Divider visible before, hidden after
 * - clearAllUnread fires on badge click / bottom-stick
 */
test.describe('UNR-001', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Unread badge clears on scroll-to-bottom @case UNR-001', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(300_000)

    const mark = `unr-${Date.now()}`

    await gotoApp(page)

    await test.step('Step 1–2: Open channel and seed scrollable content', async () => {
      await clickSidebarChannel(page, 'all')
      for (let i = 0; i < 10; i++) {
        await sendChatMessage(page, `${mark} seed message ${i}`)
      }
    })

    await test.step('Step 3: Scroll to top (away from bottom)', async () => {
      const list = page.locator('.message-list')
      await expect(list).toBeVisible()
      await list.evaluate((el) => { el.scrollTop = 0 })
    })

    await test.step('Step 4: Trigger bot reply while scrolled away (creates unread)', async () => {
      await sendChatMessage(page, `${mark} please reply with exactly: ack-${mark}`)
    })

    await test.step('Step 5: Verify NewMessageBadge appears', async () => {
      const badge = page.locator('.new-message-badge')
      await expect(badge).toBeVisible({ timeout: 120_000 })
      const badgeText = await badge.textContent()
      expect(badgeText).toMatch(/\d+ new message/)
      const countMatch = badgeText?.match(/(\d+)/)
      const initialCount = parseInt(countMatch?.[1] ?? '0', 10)
      expect(initialCount).toBeGreaterThan(0)
    })

    await test.step('Step 6: Click badge to scroll to bottom', async () => {
      await page.locator('.new-message-badge').click()
    })

    await test.step('Step 7: Verify badge and divider disappear immediately', async () => {
      await expect(page.locator('.new-message-badge')).toBeHidden({ timeout: 5000 })
      await expect(page.locator('.new-message-divider')).toBeHidden({ timeout: 5000 })
    })
  })

  test('Unread badge clears on rapid consecutive scrolls @case UNR-002', async ({ page, request }) => {
    test.skip(skipLLM, 'CHORUS_E2E_LLM=0')
    test.setTimeout(300_000)

    const mark = `unr-fast-${Date.now()}`

    await gotoApp(page)

    await test.step('Precondition: seed channel with messages and trigger replies', async () => {
      await clickSidebarChannel(page, 'all')
      for (let i = 0; i < 15; i++) {
        await sendChatMessage(page, `${mark} bulk-${i}`)
      }
      await page.locator('.message-list').evaluate((el) => { el.scrollTop = 0 })
      await sendChatMessage(page, `${mark} reply-fast: ack-fast-${mark}`)
    })

    await test.step('Wait for badge to appear', async () => {
      await expect(page.locator('.new-message-badge')).toBeVisible({ timeout: 120_000 })
    })

    await test.step('Rapidly scroll to bottom multiple times', async () => {
      const list = page.locator('.message-list')
      for (let i = 0; i < 5; i++) {
        await list.evaluate((el) => {
          el.scrollTop = el.scrollHeight
        })
        await page.waitForTimeout(100)
      }
    })

    await test.step('Verify badge gone after rapid scrolls', async () => {
      await expect(page.locator('.new-message-badge')).toBeHidden({ timeout: 5000 })
      await expect(page.locator('.new-message-divider')).toBeHidden({ timeout: 5000 })
    })
  })
})
