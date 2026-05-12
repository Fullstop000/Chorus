import { test, expect } from './helpers/fixtures'
import { gotoApp } from './helpers/ui'
import { getWhoami } from './helpers/api'

/**
 * Catalog: `qa/cases/agents.md` — ENV-001 App Startup And Identity
 *
 * Preconditions:
 * - fresh server start
 *
 * Steps:
 * 1. Open the app root URL in Chrome.
 * 2. Verify the main shell loads without a blank page or crash state.
 * 3. Verify the sidebar renders channels, agents, and humans sections.
 * 4. Verify the current user is shown in the footer.
 * 5. Verify the `whoami` value matches the visible current user.
 *
 * Expected:
 * - app loads without fatal UI error
 * - current user is stable across shell and API
 * - no immediate console exception
 */
test.describe('ENV-001', () => {
  test('App Startup And Identity @case ENV-001', async ({ page, request }) => {
    const errors: string[] = []
    page.on('console', (msg) => {
      if (msg.type() !== 'error') return
      const text = msg.text()
      // The local-mode session bootstrap (channels.ts → mintLocalSession)
      // produces an expected 401 on the very first /api/whoami before the
      // cookie is minted. The browser reports it as a "Failed to load
      // resource" console error; filter that single noise message out so
      // we still catch real app errors.
      if (text.includes('Failed to load resource: the server responded with a status of 401')) return
      errors.push(text)
    })

    await test.step('Step 1: Open the app root URL', async () => {
      await gotoApp(page)
    })

    await test.step('Step 2: Main shell loads (no blank / crash state)', async () => {
      await expect(page.locator('nav.sidebar')).toBeVisible()
    })

    await test.step('Step 3: Sidebar — Channels, Agents, Humans', async () => {
      await expect(page.locator('text=Channels').first()).toBeVisible()
      await expect(page.locator('.sidebar-section-label:text("Agents")')).toBeVisible()
      await expect(page.locator('.sidebar-section-label:text("Humans")')).toBeVisible()
    })

    await test.step('Step 4: Current user in footer', async () => {
      await expect(page.locator('.sidebar-footer')).toBeVisible()
      await expect(page.locator('.you-badge')).toBeVisible()
    })

    await test.step('Step 5: whoami matches visible user', async () => {
      const { name } = await getWhoami(request)
      await expect(page.locator('.sidebar-footer')).toContainText(name)
    })

    expect(errors, `console errors: ${errors.join('; ')}`).toEqual([])
  })
})
