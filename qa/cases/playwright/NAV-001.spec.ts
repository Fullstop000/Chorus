import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio, createChannelApi } from './helpers/api'
import { clickSidebarChannel, openAgentChat } from './helpers/ui'

/**
 * Catalog: `qa/cases/agents.md` — NAV-001 Sidebar Navigation And Selection Persistence
 */
test.describe('NAV-001', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Sidebar Navigation And Selection Persistence @case NAV-001', async ({ page, request }) => {
    const channel = await createChannelApi(request, {
      name: `qa-nav-${Date.now()}`,
      description: 'playwright NAV-001',
    })
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–4: Move between channel, agent, and tabs', async () => {
      await clickSidebarChannel(page, channel.name)
      await expect(page.locator('.chat-header-name')).toContainText(`#${channel.name}`)
      await openAgentChat(page, 'bot-a')
      await expect(page.locator('.chat-header-name')).toContainText('@bot-a')
      for (const tab of ['Profile', 'Activity', 'Workspace', 'Chat'] as const) {
        await page.getByRole('button', { name: tab, exact: true }).click()
      }
      await clickSidebarChannel(page, channel.name)
      await expect(page.locator('.chat-header-name')).toContainText(`#${channel.name}`)
    })

    await test.step('Step 5: Refresh preserves sane selected state', async () => {
      await page.reload({ waitUntil: 'networkidle' })
      const header = await page.locator('.chat-header-name').textContent()
      expect(header === `#${channel.name}` || header === '#all').toBe(true)
    })
  })
})
