import { definePlaceholderCase } from './helpers/placeholders'

/**
 * Catalog: `qa/cases/shared_memory.md` — MEM-010 No Re-Explanation In Chat When Shared Memory Exists
 */
definePlaceholderCase({
  id: 'MEM-010',
  title: 'No Re-Explanation In Chat When Shared Memory Exists',
  catalogPath: 'qa/cases/shared_memory.md',
  reason: 'Shared-memory no-re-explanation coverage is not automated yet.',
})
