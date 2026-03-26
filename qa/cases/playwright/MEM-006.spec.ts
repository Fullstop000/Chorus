import { test, expect } from '@playwright/test'
import { listChannelsApi } from './helpers/api'
import { clickSidebarChannel } from './helpers/ui'

/**
 * Catalog: `qa/cases/shared_memory.md` — MEM-006 Shared Memory Channel Not Listed As Regular Channel
 */
test.describe('MEM-006', () => {
  test('Shared Memory Channel Not Listed As Regular Channel @case MEM-006', async ({
    page,
    request,
  }) => {
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–3: Shared memory is visible and distinct in the sidebar', async () => {
      await clickSidebarChannel(page, 'shared-memory')
      await expect(page.locator('.sidebar-channel-badge').filter({ hasText: 'sys' }).first()).toBeVisible()
      await expect(page.locator('.chat-header-name')).toContainText('#shared-memory')
    })

    await test.step('Step 4: /api/channels excludes shared-memory from normal list', async () => {
      const channels = await listChannelsApi(request)
      expect(channels.some((channel) => channel.name === 'shared-memory')).toBe(false)
    })
  })
})
