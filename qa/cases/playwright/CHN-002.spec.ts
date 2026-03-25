import { test, expect } from '@playwright/test'
import {
  createChannelApi,
  deleteChannelApi,
  listChannelsApi,
  getWhoami,
} from './helpers/api'

/**
 * Catalog: `qa/cases/channels.md` — CHN-002 Channel Name Validation, Normalization, And Duplicate Rejection
 */
test.describe('CHN-002', () => {
  test('Channel Name Validation, Normalization, And Duplicate Rejection @case CHN-002', async ({
    page,
    request,
  }) => {
    const createdIds: string[] = []
    const rawName = `#QaMix-${Date.now()}`
    const normalizedName = rawName.replace(/^#/, '').toLowerCase()
    const { username } = await getWhoami(request)
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–2: Mixed-case + # prefix are normalized', async () => {
      const created = await createChannelApi(request, {
        name: rawName,
        description: 'playwright CHN-002',
      })
      createdIds.push(created.id)
      expect(created.name).toBe(normalizedName)
      await page.reload({ waitUntil: 'networkidle' })
      await expect(page.locator('.sidebar-item-text').filter({ hasText: normalizedName }).first()).toBeVisible()
    })

    await test.step('Step 3: Duplicate logical name rejected', async () => {
      const dup = await request.post('/api/channels', {
        data: { name: normalizedName.toUpperCase(), description: 'duplicate' },
      })
      expect(dup.ok()).toBeFalsy()
      const body = await dup.json()
      expect(String(body.error ?? '')).toMatch(/exists|unique constraint/i)
    })

    await test.step('Step 4: Invalid or empty name rejected', async () => {
      const empty = await request.post('/api/channels', {
        data: { name: '   ', description: '' },
      })
      expect(empty.ok()).toBeFalsy()
      const body = await empty.json()
      expect(String(body.error ?? '')).toMatch(/name is required/i)
    })

    await test.step('Step 5: No partial duplicate channel created', async () => {
      const channels = await listChannelsApi(request, { member: username })
      expect(channels.filter((c) => c.name === normalizedName)).toHaveLength(1)
    })

    for (const id of createdIds) await deleteChannelApi(request, id)
  })
})
