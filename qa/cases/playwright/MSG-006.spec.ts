import { test, expect } from './helpers/fixtures'
import { createAgentApi, getWhoami, sendAsUser, listAgents, findAgentByPrefix } from './helpers/api'
import { clickSidebarChannel , gotoApp } from './helpers/ui'

/**
 * Catalog: qa/cases/messaging.md — MSG-006 Clickable Mention Opens Agent Profile
 * Supersedes: MSG-012
 */
test.describe('MSG-006', () => {
  let actualName: string

  test.beforeAll(async ({ request }) => {
    let agents = await listAgents(request)
    let agent = findAgentByPrefix(agents, 'bot-a')
    if (!agent) {
      await createAgentApi(
        request,
        { name: 'bot-a', runtime: 'codex', model: 'gpt-5.4-mini' },
      )
      agents = await listAgents(request)
      agent = findAgentByPrefix(agents, 'bot-a')
    }
    if (!agent) throw new Error('Agent with prefix bot-a not found after creation')
    actualName = agent.name
  })

  test('Clickable Mention Opens Agent Profile @case MSG-006', async ({ page, request }) => {
    const { username } = await getWhoami(request)
    const mark = `msg06-${Date.now()}`

    // Pre-step: send a message with @mention via API so it appears in history
    await sendAsUser(request, username, '#all', `MSG-006 ${mark} testing @${actualName} mention`)

    await gotoApp(page)

    await test.step('Step 1: Open channel and locate message with @mention', async () => {
      await clickSidebarChannel(page, 'all')
      // Wait for message to appear
      await expect(page.locator('.message-content', { hasText: mark })).toBeVisible()
    })

    await test.step('Step 2: Verify @mention has clickable styling and cursor', async () => {
      const mention = page.locator('.mention-pill-clickable', { hasText: new RegExp(actualName) })
      await expect(mention).toBeVisible()
      await mention.hover()
      const cursor = await mention.evaluate((el) => getComputedStyle(el).cursor)
      expect(cursor).toBe('pointer')
    })

    await test.step('Step 3: Click @mention and verify Profile panel opens', async () => {
      const mention = page.locator('.mention-pill-clickable', { hasText: new RegExp(actualName) })
      await mention.click()

      // Verify Profile tab is active
      const profileTab = page.locator('[data-testid="tab-profile"], .tab-profile, [role="tab"]:has-text("profile")')
      await expect(profileTab).toHaveAttribute('data-active', 'true').catch(() => {
        // Fallback: check if profile panel is visible
        return expect(page.locator('.profile-panel')).toBeVisible()
      })
    })

    await test.step('Step 4: Verify correct agent is displayed in profile', async () => {
      const profilePanel = page.locator('.profile-panel')
      await expect(profilePanel).toContainText(actualName)
      
      const profileHandle = profilePanel.locator('.profile-handle')
      await expect(profileHandle).toContainText(`@${actualName}`)
    })
  })

  test('Non-existent agent mention is not clickable @case MSG-006', async ({ page, request }) => {
    const { username } = await getWhoami(request)
    const mark = `msg06-nonexist-${Date.now()}`

    // Send message with non-existent agent mention
    await sendAsUser(request, username, '#all', `MSG-006 ${mark} mentioning @nonexistent-agent`)

    await gotoApp(page)
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
