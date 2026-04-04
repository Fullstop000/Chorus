import { defineConfig, devices } from '@playwright/test'

/**
 * Playwright specs live alongside this config under `qa/cases/playwright/`.
 * Catalog: `qa/cases/*.md` — each automated case lists `- Script: playwright/<ID>.spec.ts`.
 *
 * By default each worker starts its own `chorus` process (port 3200+workerIndex,
 * isolated temp data dir) via the fixture in helpers/fixtures.ts.
 *
 * Env:
 *   CHORUS_BASE_URL   — point all workers at an existing server (disables per-worker isolation)
 *   CHORUS_E2E_LLM=0  — skip tests that wait on real agent replies
 *   CHORUS_WORKERS    — number of parallel workers (default 4)
 */
const stubE2E = process.env.CHORUS_E2E_LLM === 'stub'

export default defineConfig({
  testDir: '.',
  testMatch: '*.spec.ts',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CHORUS_WORKERS ? parseInt(process.env.CHORUS_WORKERS) : 4,
  // Stub runs wait on agent polls + slow UI; 180s is too tight for fixtures + body.
  timeout: stubE2E ? 600_000 : 180_000,
  expect: { timeout: 15_000 },
  use: {
    trace: 'on-first-retry',
    ...devices['Desktop Chrome'],
  },
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],
})
