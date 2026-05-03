import { test, expect } from './helpers/fixtures'
import { gotoApp } from './helpers/ui'
import { ensureMixedRuntimeTrio } from "./helpers/api";
import { createUserChannelViaUi, clickSidebarChannel } from "./helpers/ui";

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
 * 4. Click the card to open TaskDetail.
 * 5. Click `Start` in TaskDetail to claim + advance.
 * 6. Return to the board via the back button.
 * 7. Verify the card has moved to `in_progress`.
 *
 * Expected:
 * - state change succeeds without server error; card moves once; UI matches backend
 */
test.describe("TSK-001", () => {
  test.beforeAll(async ({ request }) => {
    await ensureMixedRuntimeTrio(request);
  });

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

    await test.step("Steps 2–3: Create task; appears in To Do", async () => {
      await page.locator(".new-task-input").fill(title);
      await page.locator(".new-task-submit").click();
      await expect(
        page
          .locator('.task-column[data-status="todo"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible();
    });

    await test.step("Steps 4–5: Click card, then Start in TaskDetail", async () => {
      await page
        .locator(".task-card")
        .filter({ hasText: title })
        .first()
        .click();
      await expect(page.locator('[data-testid="task-detail"]')).toBeVisible();
      await page.getByRole("button", { name: "Start", exact: true }).click();
      // Status pill should flip once the refetch lands.
      await expect(
        page.locator(".task-detail__status").filter({ hasText: "in progress" }),
      ).toBeVisible({ timeout: 15_000 });
    });

    await test.step("Steps 6–7: Back to board; card is in In Progress", async () => {
      await page.getByRole("button", { name: "back to channel" }).click();
      await expect(
        page
          .locator('.task-column[data-status="in_progress"] .task-card-title')
          .filter({ hasText: title }),
      ).toBeVisible({ timeout: 15_000 });
      expect(failed, failed.join("; ")).toEqual([]);
    });
  });
});
