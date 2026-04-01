import type { Page } from '@playwright/test'
import { expect } from '@playwright/test'

/**
 * Wait for the app shell to finish loading: sidebar must have at least one
 * visible item.  Always cheaper than waitUntil:'networkidle' and explicitly
 * tests a real UI signal instead of network heuristics.
 */
export async function waitForAppReady(page: Page): Promise<void> {
  await expect(page.locator('.sidebar-item-text').first()).toBeVisible({ timeout: 30_000 })
}

/** Navigate to the app root and wait for the shell to be ready. */
export async function gotoApp(page: Page): Promise<void> {
  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await waitForAppReady(page)
}

/** Reload the page and wait for the shell to be ready. */
export async function reloadApp(page: Page): Promise<void> {
  await page.reload({ waitUntil: 'domcontentloaded' })
  await waitForAppReady(page)
}

export async function createAgentViaUi(
  page: Page,
  opts: { name: string; runtime: string; model: string; reasoningEffort?: string | null }
): Promise<void> {
  await page.click('button[title="Create agent"]')
  await expect(page.locator('.modal-title:text("Create Agent")')).toBeVisible()
  await page.locator('.modal-box-agent input[placeholder="e.g. my-agent"]').fill(opts.name)
  await page.locator('.modal-box-agent .modal-field:has-text("Runtime") select').selectOption(opts.runtime)
  await page.locator('.modal-box-agent .modal-field:has-text("Model") select').first().selectOption(opts.model)
  if (opts.runtime === 'codex' && opts.reasoningEffort) {
    await page.locator('.modal-box-agent .modal-field:has-text("Reasoning") select').selectOption(opts.reasoningEffort)
  }
  await page.locator('.modal-box-agent button:has-text("Create Agent")').click()
  await expect(page.locator('.modal-title:text("Create Agent")')).toBeHidden({ timeout: 120_000 })
}

export async function createUserChannelViaUi(
  page: Page,
  name: string,
  description: string
): Promise<void> {
  await page.click('button[title="Add channel"]')
  await expect(page.locator('.modal-title:text("Create Channel")')).toBeVisible()
  await page.locator('input[placeholder="e.g. engineering"]').fill(name)
  await page.locator('input[placeholder="What\'s this channel about?"]').fill(description)
  await page.locator('.modal-card button:has-text("Create Channel")').click()
  await expect(page.locator('.modal-title:text("Create Channel")')).toBeHidden({ timeout: 30_000 })
}

/** Catalog TMT-001 steps 3–4: Leader+Operators `qa-eng`, bot-a leader, bot-b operator. */
export async function createTeamQaEngViaUi(page: Page): Promise<void> {
  await page.click('button[title="Add channel"]')
  await page.locator('.btn-brutal:has-text("Team")').click()
  await expect(page.locator('.modal-title:text("Create Team")')).toBeVisible()
  await page.locator('input[placeholder="e.g. eng-team"]').fill('qa-eng')
  await page.locator('input[placeholder="Engineering Team"]').fill('QA Engineering')
  const memberSelect = page.locator('.form-group:has-text("Initial Members") select.form-select').first()
  await memberSelect.selectOption('bot-a')
  await page.locator('.form-group:has-text("Initial Members") button:has-text("Add")').click()
  await memberSelect.selectOption('bot-b')
  await page.locator('.form-group:has-text("Initial Members") button:has-text("Add")').click()
  await page
    .locator('.form-group:has(> .form-label:text("Leader")) > select.form-select')
    .selectOption('bot-a')
  await page.locator('.modal-card button:has-text("Create Team")').click()
  await expect(page.locator('.modal-title:text("Create Team")')).toBeHidden({ timeout: 60_000 })
}

export async function clickSidebarChannel(page: Page, channelName: string): Promise<void> {
  await page.locator('.sidebar-item-text').filter({ hasText: channelName }).first().click()
}

/** Open DM / agent chat: Agents section row for `agentName`. */
export async function openAgentChat(page: Page, agentName: string): Promise<void> {
  await page
    .locator('.sidebar-section')
    .filter({ hasText: 'Agents' })
    .locator('.sidebar-item')
    .filter({ hasText: agentName })
    .first()
    .click()
}

export async function openAgent(page: Page, agentName: string): Promise<void> {
  await openAgentChat(page, agentName)
}

export async function openAgentTab(
  page: Page,
  agentName: string,
  tab: 'Chat' | 'Tasks' | 'Profile' | 'Activity' | 'Workspace'
): Promise<void> {
  await openAgent(page, agentName)
  await page.getByRole('button', { name: tab, exact: true }).click()
}

export async function sendChatMessage(page: Page, text: string): Promise<void> {
  const ta = page.locator('.message-input-textarea')
  await ta.fill(text)
  await page.locator('.message-input-send').click()
}

export async function sendThreadMessage(page: Page, text: string): Promise<void> {
  const ta = page.locator('.thread-input-textarea')
  await ta.fill(text)
  await page.locator('.thread-send-btn').click()
}

export async function openMembersPanel(page: Page): Promise<void> {
  await page.getByRole('button', { name: /Show members list/i }).click()
  await expect(page.locator('.members-panel-kicker:text("Members")')).toBeVisible()
}

export async function closeMembersPanel(page: Page): Promise<void> {
  const close = page.locator('.members-panel-close').first()
  if (await close.isVisible().catch(() => false)) {
    await close.click()
  }
}

export async function openThreadFromMessage(page: Page, contentSnippet: string): Promise<void> {
  const msg = page.locator('.message-item').filter({ hasText: contentSnippet }).first()
  await expect(msg).toBeVisible()
  await msg.hover()
  await expect(msg.locator('.message-action-btn[title="Reply in thread"]')).toBeVisible()
  await msg.locator('.message-action-btn[title="Reply in thread"]').click()
  await expect(page.locator('.thread-panel')).toBeVisible()
}
