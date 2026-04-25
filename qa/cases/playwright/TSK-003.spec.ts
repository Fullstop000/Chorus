import { test, expect } from './helpers/fixtures'
import {
  gotoApp,
  createUserChannelViaUi,
  clickSidebarChannel,
} from './helpers/ui'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-003 Task Sub-Channel Lifecycle
 *
 * Verifies the end-to-end task sub-channel story under the unified lifecycle:
 *   - sub-channels stay hidden from the sidebar
 *   - the embedded chat works from TaskDetail
 *   - status advances through claim → start → send for review → mark done,
 *     driven by the parent-channel TaskCard CTAs
 *   - Done collapses to the compact `task-card-done-pill` row
 *   - sub-channel message history survives the Done transition
 */
test.describe('TSK-003', () => {
  test('Task Sub-Channel Lifecycle @case TSK-003', async ({ page }) => {
    const slug = `qa-task-life-${Date.now()}`
    const title = `TSK-003 ${Date.now()}`
    const subChannelMessage = `sub-channel msg ${Date.now()}`

    const failed: string[] = []
    page.on('response', (res) => {
      const url = res.url()
      if (
        (url.includes('/tasks') || url.includes('/channels')) &&
        res.status() >= 400
      ) {
        failed.push(`${res.status()} ${url}`)
      }
    })

    await gotoApp(page)

    await test.step('Step 1: Create a channel', async () => {
      await createUserChannelViaUi(page, slug, 'playwright TSK-003')
      await clickSidebarChannel(page, slug)
    })

    await test.step('Step 2: Create a task with a unique title', async () => {
      await page.getByRole('button', { name: 'Tasks', exact: true }).click()
      await page.locator('.new-task-input').fill(title)
      await page.locator('.new-task-submit').click()
      await expect(
        page
          .locator('.task-column[data-status="todo"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible()
    })

    await test.step('Step 3: Task sub-channel is not visible in the sidebar', async () => {
      // Sub-channels follow the `<parent>__task-N` slug pattern. We don't need
      // to know the number — assert no sidebar entry surfaces the pattern.
      const taskishSidebarItems = page
        .locator('.sidebar-item-text')
        .filter({ hasText: /__task-\d+/ })
      await expect(taskishSidebarItems).toHaveCount(0)
    })

    // Locate the parent-channel TaskCard host message; status changes will
    // mutate this same node in place via the `task_update` SSE stream.
    const card = page
      .locator('[data-testid^="task-card-"]')
      .filter({ hasText: title })
      .first()

    await test.step('Step 4: Chat shows the parent-channel TaskCard in todo', async () => {
      await page.getByRole('button', { name: 'Chat', exact: true }).click()
      await expect(card).toBeVisible({ timeout: 15_000 })
      await expect(card).toHaveAttribute('data-status', 'todo')
    })

    await test.step('Step 5: Claim then start via the card; status flips to in_progress', async () => {
      await card.locator('[data-testid="task-card-claim-btn"]').click()
      await expect(card).toHaveAttribute('data-claimed', 'true', { timeout: 15_000 })
      await card.locator('[data-testid="task-card-start-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'in_progress', { timeout: 15_000 })
    })

    await test.step('Step 6: Open task detail via the deep-link; embedded chat works', async () => {
      await card.locator('[data-testid="task-card-link"]').click()
      await expect(page.locator('[data-testid="task-detail"]')).toBeVisible()
      // STATUS_LABEL renames `in_progress` → "in progress" for display.
      await expect(
        page.locator('.task-detail__status').filter({ hasText: 'in progress' }),
      ).toBeVisible({ timeout: 15_000 })

      const ta = page
        .locator('[data-testid="task-detail"] .message-input-textarea')
        .first()
      await ta.fill(subChannelMessage)
      await ta.press('Enter')
      await expect(
        page
          .locator('[data-testid="task-detail"] .message-item')
          .filter({ hasText: subChannelMessage })
          .first(),
      ).toBeVisible({ timeout: 15_000 })

      await page.getByRole('button', { name: 'back to channel' }).click()
    })

    await test.step('Step 7: Send for review via the card; data-status flips to in_review', async () => {
      await card.locator('[data-testid="task-card-review-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'in_review', { timeout: 15_000 })
    })

    await test.step('Step 8: Mark done; card collapses to task-card-done-pill', async () => {
      await card.locator('[data-testid="task-card-done-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'done', { timeout: 15_000 })
      await expect(card.locator('[data-testid="task-card-done-pill"]')).toBeVisible()
      await expect(card).toContainText(title)
    })

    await test.step('Step 9: Done column has the task; sub-channel history survives', async () => {
      await page.getByRole('button', { name: 'Tasks', exact: true }).click()
      await expect(
        page
          .locator('.task-column[data-status="done"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible({ timeout: 15_000 })

      await page
        .locator('.task-card')
        .filter({ hasText: title })
        .first()
        .click()
      await expect(page.locator('[data-testid="task-detail"]')).toBeVisible()
      await expect(
        page
          .locator('[data-testid="task-detail"] .message-item')
          .filter({ hasText: subChannelMessage })
          .first(),
      ).toBeVisible({ timeout: 15_000 })

      expect(failed, failed.join('; ')).toEqual([])
    })
  })
})
