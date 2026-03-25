import { definePlaceholderCase } from './helpers/placeholders'

/**
 * Catalog: `qa/cases/agents.md` — REC-001 Restart Server And Verify Agent Recovery
 */
definePlaceholderCase({
  id: 'REC-001',
  title: 'Restart Server And Verify Agent Recovery',
  catalogPath: 'qa/cases/agents.md',
  reason: 'Restart-session recovery coverage is not automated yet.',
})
