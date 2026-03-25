import { test, expect } from '@playwright/test'
import { ensureMixedRuntimeTrio } from './helpers/api'
import { createUserChannelViaUi, clickSidebarChannel } from './helpers/ui'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-001 Create And Advance A Task
 *
 * Preconditions:
 * - tasks tab available
 *
 * Steps:
 * 1. Open `Tasks`.
 * 2. Create a new task with an unambiguous title.
 * 3. Verify it appears in `To Do`.
 * 4. Advance it once.
 * 5. Verify it moves to the correct next state.
 * 6. Watch console and network responses during the transition.
 *
 * Expected:
 * - state change succeeds without server error; card moves once; UI matches backend
 */
test.describe('TSK-001', () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request)
  })

  test('Create And Advance A Task @case TSK-001', async ({ page }) => {
    const slug = `qa-tasks-${Date.now()}`
    const title = `TSK-001 ${Date.now()}`
    const failed: string[] = []
    page.on('response', (res) => {
      if (res.url().includes('/tasks') && res.status() >= 400) {
        failed.push(`${res.status()} ${res.url()}`)
      }
    })

    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Step 1: Open Tasks on a channel', async () => {
      await createUserChannelViaUi(page, slug, 'playwright TSK-001')
      await clickSidebarChannel(page, slug)
      await page.getByRole('button', { name: 'Tasks' }).click()
    })

    await test.step('Steps 2–3: Create task; appears in To Do', async () => {
      await page.locator('.new-task-input').fill(title)
      await page.locator('.new-task-submit').click()
      await expect(page.locator('.task-column[data-status="todo"] .task-card-title').filter({ hasText: title })).toBeVisible()
    })

    await test.step('Steps 4–6: Advance once; no error responses', async () => {
      await page.locator('.task-card').filter({ hasText: title }).first().click()
      await expect(
        page.locator('.task-column[data-status="in_progress"] .task-card-title').filter({ hasText: title })
      ).toBeVisible({ timeout: 15_000 })
      expect(failed, failed.join('; ')).toEqual([])
    })
  })
})
