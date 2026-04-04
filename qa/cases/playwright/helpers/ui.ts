import type { Page } from '@playwright/test'
import { expect } from '@playwright/test'

import { agentNames } from './api'

/**
 * Wait for the app shell to finish loading: sidebar must have at least one
 * visible item.  Always cheaper than waitUntil:'networkidle' and explicitly
 * tests a real UI signal instead of network heuristics.
 */
export async function waitForAppReady(page: Page): Promise<void> {
  const stub = process.env.CHORUS_E2E_LLM === 'stub'
  await expect(page.locator('.sidebar-item-text').first()).toBeVisible({
    timeout: stub ? 90_000 : 30_000,
  })
}

/** Navigate to the app root and wait for the shell to be ready. */
export async function gotoApp(page: Page): Promise<void> {
  const stub = process.env.CHORUS_E2E_LLM === 'stub'
  const attempts = stub ? 2 : 1
  let lastErr: unknown
  for (let i = 0; i < attempts; i++) {
    await page.goto('/', { waitUntil: 'domcontentloaded' })
    try {
      await waitForAppReady(page)
      return
    } catch (e) {
      lastErr = e
      if (i < attempts - 1) {
        await new Promise((r) => setTimeout(r, 1_500))
      }
    }
  }
  throw lastErr
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
  const dialog = page.locator('[role="dialog"]')
  await expect(dialog.getByRole('heading', { name: 'Create Agent' })).toBeVisible()
  await dialog.locator('input[placeholder="e.g. my-agent"]').fill(opts.name)
  await dialog.locator('[role="combobox"][aria-label="Runtime"]').click()
  await page.locator('[role="option"]').filter({ hasText: new RegExp(opts.runtime, 'i') }).first().click()
  await dialog.locator('[role="combobox"][aria-label="Model"]').click()
  // Substring match would pick e.g. `gpt-5.2-codex` when asking for `gpt-5.2`.
  await page.getByRole('option', { name: opts.model, exact: true }).click()
  if (opts.runtime === 'codex' && opts.reasoningEffort) {
    await dialog.locator('[role="combobox"][aria-label="Reasoning"]').click()
    await page.locator('[role="option"]').filter({ hasText: new RegExp(opts.reasoningEffort, 'i') }).first().click()
  }
  await dialog.locator('button:has-text("Create Agent")').click()
  await expect(dialog).toBeHidden({ timeout: 120_000 })
}

export async function createUserChannelViaUi(
  page: Page,
  name: string,
  description: string
): Promise<void> {
  await page.click('button[title="Add channel"]')
  const dialog = page.locator('[role="dialog"]')
  await expect(dialog.getByRole('heading', { name: 'Create Channel' })).toBeVisible()
  await page.locator('input[placeholder="e.g. engineering"]').fill(name)
  await page.locator('input[placeholder="What\'s this channel about?"]').fill(description)
  await dialog.locator('button:has-text("Create Channel")').click()
  await expect(dialog).toBeHidden({ timeout: 30_000 })
}

/** Catalog TMT-001 steps 3–4: Leader+Operators `qa-eng`, bot-a leader, bot-b operator. */
export async function createTeamQaEngViaUi(page: Page): Promise<void> {
  const { a, b } = agentNames()
  await page.click('button[title="Add channel"]')
  const dialog = page.locator('[role="dialog"]')
  await dialog.locator('button:has-text("Team")').click()
  await expect(dialog.getByRole('heading', { name: 'Create Team' })).toBeVisible()
  await page.locator('input[placeholder="e.g. eng-team"]').fill('qa-eng')
  await page.locator('input[placeholder="Engineering Team"]').fill('QA Engineering')
  const memberSelect = dialog.locator('[role="combobox"][aria-label="Initial Members"]')
  await memberSelect.click()
  await page.locator('[role="option"]').filter({ hasText: a }).first().click()
  await dialog.locator('button:has-text("Add")').click()
  await memberSelect.click()
  await page.locator('[role="option"]').filter({ hasText: b }).first().click()
  await dialog.locator('button:has-text("Add")').click()
  await dialog.locator('[role="combobox"][aria-label="Leader"]').click()
  await page.locator('[role="option"]').filter({ hasText: a }).first().click()
  await dialog.locator('button:has-text("Create Team")').click()
  await expect(dialog).toBeHidden({ timeout: 60_000 })
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
  const kicker = page.locator('.members-panel-kicker:text("Members")')
  if (!(await kicker.isVisible().catch(() => false))) {
    await page.getByRole('button', { name: 'Show members list' }).click()
  }
  await expect(kicker).toBeVisible()
}

/** Radix options in portaled popovers may still be "outside viewport" for Playwright hit-testing. */
export async function clickComboboxOption(page: Page, optionText: string): Promise<void> {
  const opt = page.locator('[role="option"]').filter({ hasText: optionText }).first()
  await opt.waitFor({ state: 'attached', timeout: 15_000 })
  await opt.evaluate((el) => (el as HTMLElement).click())
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
