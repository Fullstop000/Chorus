import { test, expect } from './helpers/fixtures'
import {
  gotoApp,
  createUserChannelViaUi,
  clickSidebarChannel,
} from './helpers/ui'

/**
 * Catalog: `qa/cases/tasks.md` — TSK-006 Full Lifecycle Smoke
 *
 * One human, one direct-created task, full forward-only state machine driven
 * entirely from the parent-channel `TaskCard`:
 *
 *   todo (unowned) → claim → todo (owned) → start → in_progress → review →
 *   in_review → done → collapsed pill → click pill → sub-channel opens.
 *
 * Distinct from TSK-001 (which validates kanban ↔ chat parity) by exercising
 * every CTA branch on a single card without round-tripping through the kanban
 * board, and from TSK-003 (sub-channel chat survival) by skipping the embedded
 * chat write — this is a state-transition smoke, end to end.
 */
test.describe('TSK-006', () => {
  test('Full Lifecycle Smoke @case TSK-006', async ({ page }) => {
    const slug = `qa-task-smoke-${Date.now()}`
    const title = `TSK-006 ${Date.now()}`

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

    await test.step('Step 1: Create a parent channel and a task', async () => {
      await createUserChannelViaUi(page, slug, 'playwright TSK-006')
      await clickSidebarChannel(page, slug)
      await page.getByRole('button', { name: 'Tasks', exact: true }).click()
      await page.locator('.new-task-input').fill(title)
      await page.locator('.new-task-submit').click()
      await expect(
        page
          .locator('.task-column[data-status="todo"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible()
    })

    // Switching to Chat surfaces the parent-channel `task_card` host message.
    // The kanban hop above also primed the global tasksStore via useTasks
    // polling, so `useTask(taskId)` resolves to a real row.
    const card = page
      .locator('[data-testid^="task-card-"]')
      .filter({ hasText: title })
      .first()

    await test.step('Step 2: Chat shows the card in todo with [claim] CTA', async () => {
      await page.getByRole('button', { name: 'Chat', exact: true }).click()
      await expect(card).toBeVisible({ timeout: 15_000 })
      await expect(card).toHaveAttribute('data-status', 'todo')
      await expect(card).toHaveAttribute('data-claimed', 'false')
      await expect(card.locator('[data-testid="task-card-claim-btn"]')).toBeVisible()
    })

    await test.step('Step 3: [claim] → owner badge + [start] CTA', async () => {
      await card.locator('[data-testid="task-card-claim-btn"]').click()
      await expect(card).toHaveAttribute('data-claimed', 'true', { timeout: 15_000 })
      // Claim is decoupled from status — still on todo.
      await expect(card).toHaveAttribute('data-status', 'todo')
      await expect(card).toContainText(/claimed by @/)
      await expect(card.locator('[data-testid="task-card-start-btn"]')).toBeVisible()
    })

    await test.step('Step 4: [start] → in_progress + [send for review] CTA', async () => {
      await card.locator('[data-testid="task-card-start-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'in_progress', { timeout: 15_000 })
      await expect(card.locator('[data-testid="task-card-review-btn"]')).toBeVisible()
    })

    await test.step('Step 5: [send for review] → in_review + [mark done] CTA', async () => {
      await card.locator('[data-testid="task-card-review-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'in_review', { timeout: 15_000 })
      await expect(card.locator('[data-testid="task-card-done-btn"]')).toBeVisible()
    })

    await test.step('Step 6: [mark done] → card collapses to done pill', async () => {
      await card.locator('[data-testid="task-card-done-btn"]').click()
      await expect(card).toHaveAttribute('data-status', 'done', { timeout: 15_000 })
      await expect(card.locator('[data-testid="task-card-done-pill"]')).toBeVisible()
    })

    await test.step('Step 7: Click pill → sub-channel opens via TaskDetail', async () => {
      await card.locator('[data-testid="task-card-done-pill"]').click()
      await expect(page.locator('[data-testid="task-detail"]')).toBeVisible({ timeout: 15_000 })
    })

    expect(failed, failed.join('; ')).toEqual([])
  })
})
