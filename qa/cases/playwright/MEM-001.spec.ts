import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser, rememberApi } from './helpers/api'
import { clickSidebarChannel } from './helpers/ui'

/**
 * Catalog: `qa/cases/shared_memory.md` — MEM-001 Remember And Breadcrumb Visibility
 */
test.describe('MEM-001', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Remember And Breadcrumb Visibility @case MEM-001', async ({ page, request }) => {
    const token = `mem-check-${Date.now()}`
    const { username } = await getWhoami(request)
    await rememberApi(request, 'bot-a', {
      key: 'test-finding',
      value: token,
      tags: ['qa'],
      channelContext: 'all',
    })
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 4–8: Breadcrumb appears in #shared-memory and survives refresh', async () => {
      await clickSidebarChannel(page, 'shared-memory')
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
      await page.reload({ waitUntil: 'networkidle' })
      await clickSidebarChannel(page, 'shared-memory')
      await expect(page.locator('.message-item').filter({ hasText: token }).first()).toBeVisible()
      const history = await historyForUser(request, username, '#shared-memory', 30)
      expect(history.some((m) => (m.content ?? '').includes(token))).toBe(true)
    })
  })
})
