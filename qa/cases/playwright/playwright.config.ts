import { defineConfig, devices } from '@playwright/test'

/**
 * Playwright specs live alongside this config under `qa/cases/playwright/`.
 * Catalog: `qa/cases/*.md` — each automated case lists `- Script: playwright/<ID>.spec.ts`.
 *
 * Prereq: `ui/dist` built + `chorus serve` (see `qa/README.md`).
 *
 * Env: `CHORUS_BASE_URL`, `CHORUS_E2E_LLM=0` to skip LLM-wait tests.
 */
const baseURL = process.env.CHORUS_BASE_URL ?? 'http://localhost:3101'

export default defineConfig({
  testDir: '.',
  testMatch: '*.spec.ts',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  timeout: 180_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL,
    trace: 'on-first-retry',
    ...devices['Desktop Chrome'],
  },
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],
})
