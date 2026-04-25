import { test, expect } from './helpers/fixtures'
import { gotoApp } from './helpers/ui'
import { createUserChannelViaUi, clickSidebarChannel } from "./helpers/ui";

/**
 * Catalog: `qa/cases/tasks.md` — TSK-001 Create And Advance A Task
 *
 * Preconditions:
 * - tasks tab available
 *
 * Steps:
 * 1. Open `Tasks` on a fresh channel.
 * 2. Create a new task with an unambiguous title; verify it appears in `To Do`.
 * 3. Switch to `Chat`; verify the parent-channel TaskCard renders in `todo`.
 * 4. Click `[claim]` on the card; verify owner badge appears.
 * 5. Click `[start]` on the card; verify status flips to `in_progress`.
 * 6. Re-open `Tasks`; verify the card has moved to `In Progress`.
 *
 * Expected:
 * - claim and start are two separate user actions on the parent-channel card;
 *   each one updates the same card in place; UI matches backend.
 */
test.describe("TSK-001", () => {
  test("Create And Advance A Task @case TSK-001", async ({ page }) => {
    const slug = `qa-tasks-${Date.now()}`;
    const title = `TSK-001 ${Date.now()}`;
    const failed: string[] = [];
    page.on("response", (res) => {
      if (res.url().includes("/tasks") && res.status() >= 400) {
        failed.push(`${res.status()} ${res.url()}`);
      }
    });

    await gotoApp(page)

    await test.step("Step 1: Open Tasks on a channel", async () => {
      await createUserChannelViaUi(page, slug, "playwright TSK-001");
      await clickSidebarChannel(page, slug);
      await page.getByRole("button", { name: "Tasks", exact: true }).click();
    });

    await test.step("Step 2: Create task; appears in To Do", async () => {
      await page.locator(".new-task-input").fill(title);
      await page.locator(".new-task-submit").click();
      await expect(
        page
          .locator('.task-column[data-status="todo"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible();
    });

    // Visiting the Tasks tab populates the tasksStore via useTasks polling, so
    // the TaskCard host message in chat can resolve `useTask(taskId)` to the
    // freshly created row. Without this hop the card would render as null.
    const card = page
      .locator('[data-testid^="task-card-"]')
      .filter({ hasText: title })
      .first();

    await test.step("Step 3: Chat shows the parent-channel TaskCard in todo", async () => {
      await page.getByRole("button", { name: "Chat", exact: true }).click();
      await expect(card).toBeVisible({ timeout: 15_000 });
      await expect(card).toHaveAttribute("data-status", "todo");
      await expect(card).toHaveAttribute("data-claimed", "false");
    });

    await test.step("Step 4: Click [claim]; owner badge appears", async () => {
      await card.locator('[data-testid="task-card-claim-btn"]').click();
      // Claim is decoupled from status — card stays on `todo`, but `data-claimed`
      // flips and the start CTA replaces the claim CTA.
      await expect(card).toHaveAttribute("data-claimed", "true", {
        timeout: 15_000,
      });
      await expect(card).toContainText(/claimed by @/);
      await expect(
        card.locator('[data-testid="task-card-start-btn"]'),
      ).toBeVisible();
    });

    await test.step("Step 5: Click [start]; status flips to in_progress", async () => {
      await card.locator('[data-testid="task-card-start-btn"]').click();
      await expect(card).toHaveAttribute("data-status", "in_progress", {
        timeout: 15_000,
      });
    });

    await test.step("Step 6: Tasks board shows the card in In Progress", async () => {
      await page.getByRole("button", { name: "Tasks", exact: true }).click();
      await expect(
        page
          .locator('.task-column[data-status="in_progress"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible({ timeout: 15_000 });
      expect(failed, failed.join("; ")).toEqual([]);
    });
  });
});
