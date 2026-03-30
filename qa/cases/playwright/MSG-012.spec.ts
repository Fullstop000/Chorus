import { test, expect } from '@playwright/test'
import { createAgentApi, getWhoami, sendAsUser } from './helpers/api'
import { clickSidebarChannel } from './helpers/ui'

/**
 * Catalog: `qa/cases/messaging.md` — MSG-012 Clickable Mention Opens Agent Profile
 *
 * Preconditions:
 * - `bot-a` exists
 * - active test channel is open
 *
 * Steps:
 * 1. Send a message as human containing `@bot-a` mention.
 * 2. Locate the message and verify @mention has clickable styling.
 * 3. Hover over the @mention and verify cursor changes to pointer.
 * 4. Click the @mention.
 * 5. Verify the Profile panel opens with the correct agent.
 *
 * Expected:
 * - @mention renders with distinct pill styling
 * - clickable @mentions show pointer cursor on hover
 * - clicking opens Profile tab with correct agent selected
 * - non-existent agent mentions are not clickable
 */
test.describe('MSG-012', () => {
  test.beforeAll(async ({ request }) => {
    await createAgentApi(request, { name: 'bot-a', runtime: 'claude', model: 'sonnet' })
  })

  test('Clickable Mention Opens Agent Profile @case MSG-012', async ({ page, request }) => {
    const { username } = await getWhoami(request)
    const mark = `msg12-${Date.now()}`

    // Pre-step: send a message with @mention via API so it appears in history
    await sendAsUser(request, username, '#all', `MSG-012 ${mark} testing @bot-a mention`)

    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Step 1: Open channel and locate message with @mention', async () => {
      await clickSidebarChannel(page, 'all')
      // Wait for message to appear
      await expect(page.locator('.message-content', { hasText: mark })).toBeVisible()
    })

    await test.step('Step 2: Verify @mention has clickable styling and cursor', async () => {
      const mention = page.locator('.mention-pill-clickable', { hasText: /@bot-a/ })
      await expect(mention).toBeVisible()
      
      // Hover and verify cursor changes
      await mention.hover()
      const cursor = await mention.evaluate((el) => getComputedStyle(el).cursor)
      expect(cursor).toBe('pointer')
    })

    await test.step('Step 3: Click @mention and verify Profile panel opens', async () => {
      const mention = page.locator('.mention-pill-clickable', { hasText: /@bot-a/ })
      await mention.click()

      // Verify Profile tab is active
      const profileTab = page.locator('[data-testid="tab-profile"], .tab-profile, [role="tab"]:has-text("profile")')
      await expect(profileTab).toHaveAttribute('data-active', 'true').catch(() => {
        // Fallback: check if profile panel is visible
        return expect(page.locator('.profile-panel')).toBeVisible()
      })
    })

    await test.step('Step 4: Verify correct agent is displayed in profile', async () => {
      // Verify profile shows bot-a
      const profilePanel = page.locator('.profile-panel')
      await expect(profilePanel).toContainText('bot-a')
      
      // Verify profile handle shows @bot-a
      const profileHandle = profilePanel.locator('.profile-handle')
      await expect(profileHandle).toContainText('@bot-a')
    })
  })

  test('Non-existent agent mention is not clickable @case MSG-012', async ({ page, request }) => {
    const { username } = await getWhoami(request)
    const mark = `msg12-nonexist-${Date.now()}`

    // Send message with non-existent agent mention
    await sendAsUser(request, username, '#all', `MSG-012 ${mark} mentioning @nonexistent-agent`)

    await page.goto('/', { waitUntil: 'networkidle' })
    await clickSidebarChannel(page, 'all')
    // Wait for message to appear
    await expect(page.locator('.message-content', { hasText: mark })).toBeVisible()

    await test.step('Verify non-existent agent mention is not clickable', async () => {
      // Should have mention-pill class but NOT mention-pill-clickable
      const mention = page.locator('.mention-pill', { hasText: /@nonexistent-agent/ })
      await expect(mention).toBeVisible()
      
      // Should not have clickable class
      const clickableMention = page.locator('.mention-pill-clickable', { hasText: /@nonexistent-agent/ })
      await expect(clickableMention).not.toBeVisible()
      
      // Cursor should not be pointer
      const cursor = await mention.evaluate((el) => getComputedStyle(el).cursor)
      expect(cursor).not.toBe('pointer')
    })
  })
})
