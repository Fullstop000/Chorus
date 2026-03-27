import { test, expect } from '@playwright/test'
import { createAgentViaUi, openAgentTab } from './helpers/ui'
import { getAgentDetail, listAgents } from './helpers/api'

const MODELS: Record<string, string[]> = {
  claude: ['sonnet', 'opus', 'haiku'],
  codex: [
    'gpt-5.4',
    'gpt-5.4-mini',
    'gpt-5.3-codex',
    'gpt-5.2-codex',
    'gpt-5.2',
    'gpt-5.1-codex-max',
    'gpt-5.1-codex-mini',
  ],
  kimi: ['kimi-code/kimi-for-coding'],
}

/**
 * Catalog: `qa/cases/agents.md` — AGT-002 Agent Create Matrix Across Every Driver And Model
 */
test.describe('AGT-002', () => {
  test('Agent Create Matrix Across Every Driver And Model @case AGT-002', async ({ page, request }) => {
    const created: string[] = []
    await page.goto('/', { waitUntil: 'networkidle' })

    await test.step('Steps 1–9: Create one agent for every runtime/model pair and verify stored config', async () => {
      for (const runtime of Object.keys(MODELS)) {
        for (const model of MODELS[runtime]) {
          const name = `matrix-${runtime}-${model.replace(/[^a-z0-9]+/gi, '-').toLowerCase()}-${Date.now()}`
          const reasoningEffort = runtime === 'codex' ? 'high' : null
          await createAgentViaUi(page, { name, runtime, model, reasoningEffort })
          created.push(name)
          await openAgentTab(page, name, 'Profile')
          await expect(page.locator('.profile-config-grid')).toContainText(runtime)
          await expect(page.locator('.profile-config-grid')).toContainText(model)
          if (runtime === 'codex') {
            await expect(page.locator('.profile-config-grid')).toContainText('high')
          }
          const detail = await getAgentDetail(request, name)
          expect(detail.agent.runtime).toBe(runtime)
          expect(detail.agent.model).toBe(model)
        }
      }
    })

    await test.step('Steps 10–12: Duplicate names fail regardless of config', async () => {
      const dupName = created[0]
      const first = await request.post('/api/agents', {
        data: {
          name: dupName,
          display_name: dupName,
          description: 'dup',
          runtime: 'claude',
          model: 'sonnet',
          envVars: [],
        },
      })
      expect(first.ok()).toBeFalsy()
      const second = await request.post('/api/agents', {
        data: {
          name: dupName,
          display_name: dupName,
          description: 'dup',
          runtime: 'codex',
          model: 'gpt-5.4-mini',
          reasoningEffort: 'high',
          envVars: [],
        },
      })
      expect(second.ok()).toBeFalsy()
      const agents = await listAgents(request)
      expect(agents.filter((agent) => agent.name === dupName)).toHaveLength(1)
    })
  })
})
