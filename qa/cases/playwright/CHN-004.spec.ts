import { test, expect } from '@playwright/test'
import {
  createChannelApi,
  deleteChannelApi,
  getWhoami,
  sendAsUser,
} from './helpers/api'
import { clickSidebarChannel } from './helpers/ui'

/**
 * Catalog: `qa/cases/channels.md` — CHN-004 Channel Delete And Selection Recovery
 */
test.describe('CHN-004', () => {
  test('Channel Delete And Selection Recovery @case CHN-004', async ({ page, request }) => {
    const { username } = await getWhoami(request)
    const channel = await createChannelApi(request, {
      name: `qa-delete-${Date.now()}`,
      description: 'playwright CHN-004',
    })
    await sendAsUser(request, username, `#${channel.name}`, 'seed delete case')
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–3: Open disposable channel and delete it through API', async () => {
      await clickSidebarChannel(page, channel.name)
      await expect(page.locator('.chat-header-name')).toContainText(`#${channel.name}`)
      await deleteChannelApi(request, channel.id)
    })

    await test.step('Steps 4–6: Sidebar and selection recover after refresh', async () => {
      await page.reload({ waitUntil: 'networkidle' })
      await expect(page.locator('.sidebar-item-text').filter({ hasText: channel.name })).toHaveCount(0)
      await expect(page.locator('.chat-header-name')).not.toContainText(`#${channel.name}`)
    })
  })
})
