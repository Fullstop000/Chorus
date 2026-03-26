import { definePlaceholderCase } from './helpers/placeholders'

/**
 * Catalog: `qa/cases/shared_memory.md` — MEM-005 Shared Memory Survives Server Restart
 */
definePlaceholderCase({
  id: 'MEM-005',
  title: 'Shared Memory Survives Server Restart',
  catalogPath: 'qa/cases/shared_memory.md',
  reason: 'Shared-memory restart persistence coverage is not automated yet.',
})
