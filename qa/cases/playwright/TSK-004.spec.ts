import { test, expect } from './helpers/fixtures'
import {
  gotoApp,
  createUserChannelViaUi,
  clickSidebarChannel,
} from './helpers/ui'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-004 Channel Task-Event Feed
 *
 * Verifies that the parent-channel chat surfaces a task as a single living
 * `TaskCard` host message that mutates through the unified lifecycle and
 * collapses to the compact `task-card-done-pill` on Done:
 *   - direct create posts a parent-channel `task_card` system message
 *   - claim → start → review → done each update the SAME card in place
 *   - Done collapses the card to the pill view (data-status="done")
 */
test.describe('TSK-004', () => {
  test('Channel Task-Event Feed @case TSK-004', async ({ page }) => {
    const slug = `qa-task-feed-${Date.now()}`
    const title = `TSK-004 ${Date.now()}`

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
      await createUserChannelViaUi(page, slug, 'playwright TSK-004')
      await clickSidebarChannel(page, slug)
    })

    await test.step('Step 2: Create a task via the kanban tab', async () => {
      await page.getByRole('button', { name: 'Tasks', exact: true }).click()
      await page.locator('.new-task-input').fill(title)
      await page.locator('.new-task-submit').click()
      await expect(
        page
          .locator('.task-column[data-status="todo"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible()
    })

    const card = page
      .locator('[data-testid^="task-card-"]')
      .filter({ hasText: title })
      .first()

    await test.step('Step 3: Parent channel chat shows a task card in todo', async () => {
      await page.getByRole('button', { name: 'Chat', exact: true }).click()
      await expect(card).toBeVisible({ timeout: 15_000 })
      await expect(card).toHaveAttribute('data-status', 'todo')
      await expect(card).toContainText(title)
    })

    await test.step('Step 4: Claim then start; same card flips to in_progress', async () => {
      await card.locator('[data-testid="task-card-claim-btn"]').click()
      await expect(card).toHaveAttribute('data-claimed', 'true', { timeout: 15_000 })
      await card.locator('[data-testid="task-card-start-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'in_progress', { timeout: 15_000 })
    })

    await test.step('Step 5: Send for review; same card flips to in_review', async () => {
      await card.locator('[data-testid="task-card-review-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'in_review', { timeout: 15_000 })
    })

    await test.step('Step 6: Mark done; card collapses to the done pill', async () => {
      await card.locator('[data-testid="task-card-done-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'done', { timeout: 15_000 })
      await expect(card.locator('[data-testid="task-card-done-pill"]')).toBeVisible()
      await expect(card.locator('[data-testid="task-card-done-pill"]')).toContainText(title)
    })

    expect(failed, 'no 4xx/5xx responses during the lifecycle').toEqual([])
  })
})
