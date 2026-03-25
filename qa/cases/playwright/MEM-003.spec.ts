import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio, getWhoami, historyForUser } from './helpers/api'
import { clickSidebarChannel } from './helpers/ui'

/**
 * Catalog: `qa/cases/shared_memory.md` — MEM-003 System Channel Write Guard
 */
test.describe('MEM-003', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('System Channel Write Guard @case MEM-003', async ({ page, request }) => {
    const { username } = await getWhoami(request)
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–3: Human composer is read-only in #shared-memory', async () => {
      await clickSidebarChannel(page, 'shared-memory')
      await expect(page.locator('.message-input-textarea')).toBeDisabled()
      await expect(page.locator('.message-input-send')).toBeDisabled()
    })

    await test.step('Steps 4–6: Direct send to #shared-memory is rejected and no message appears', async () => {
      const res = await request.post(`/internal/agent/${encodeURIComponent(username)}/send`, {
        data: {
          target: '#shared-memory',
          content: 'direct-post-attempt',
          attachmentIds: [],
        },
      })
      expect(res.ok()).toBeFalsy()
      const body = await res.json()
      expect(String(body.error ?? '')).toMatch(/remember/i)
      const history = await historyForUser(request, username, '#shared-memory', 30)
      expect(history.some((m) => (m.content ?? '').includes('direct-post-attempt'))).toBe(false)
    })
  })
})
