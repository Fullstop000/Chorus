import { test, expect } from './helpers/fixtures'
import {
  gotoApp,
  createUserChannelViaUi,
  clickSidebarChannel,
} from './helpers/ui'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-003 Task Sub-Channel Lifecycle
 *
 * Verifies the end-to-end task sub-channel story:
 *   - sub-channels stay hidden from the sidebar
 *   - embedded chat works from TaskDetail
 *   - status advances through Start → Submit for review → Mark done
 *   - archival on Done preserves message history
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
      // Task sub-channels use a predictable name pattern (`<parent>__task-N`),
      // but we only need to assert the child channel's slug never surfaces in
      // the sidebar. Checking for any sidebar entry containing `__task-` is a
      // more robust structural check than guessing the task number.
      const taskishSidebarItems = page
        .locator('.sidebar-item-text')
        .filter({ hasText: /__task-\d+/ })
      await expect(taskishSidebarItems).toHaveCount(0)
    })

    await test.step('Step 4: Click the card; TaskDetail renders', async () => {
      await page
        .locator('.task-card')
        .filter({ hasText: title })
        .first()
        .click()
      await expect(page.locator('[data-testid="task-detail"]')).toBeVisible()
    })

    await test.step('Step 5: Post a message in the embedded chat', async () => {
      // TaskDetail renders a single MessageInput scoped to the sub-channel —
      // `.message-input-textarea` is unique within the detail view.
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
    })

    await test.step('Step 6: Advance Start → Submit for review → Mark done', async () => {
      await page.getByRole('button', { name: 'Start', exact: true }).click()
      await expect(
        page
          .locator('.task-detail__status')
          .filter({ hasText: 'in_progress' }),
      ).toBeVisible({ timeout: 15_000 })

      await page
        .getByRole('button', { name: 'Submit for review', exact: true })
        .click()
      await expect(
        page.locator('.task-detail__status').filter({ hasText: 'in_review' }),
      ).toBeVisible({ timeout: 15_000 })

      await page
        .getByRole('button', { name: 'Mark done', exact: true })
        .click()
      await expect(
        page.locator('.task-detail__status').filter({ hasText: 'done' }),
      ).toBeVisible({ timeout: 15_000 })
    })

    await test.step('Step 7: Task is on the Done column', async () => {
      await page.getByRole('button', { name: 'back to channel' }).click()
      await expect(
        page
          .locator('.task-column[data-status="done"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible({ timeout: 15_000 })
    })

    await test.step('Step 8: Reopen task detail; posted message still visible', async () => {
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
