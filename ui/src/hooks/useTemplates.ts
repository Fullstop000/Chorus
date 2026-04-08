import { useEffect, useState } from 'react'
import { listTemplates } from '../data/templates'
import type { AgentTemplate, TemplateCategory } from '../data/templates'

export type { AgentTemplate, TemplateCategory }

export function useTemplates(): {
  categories: TemplateCategory[]
  allTemplates: AgentTemplate[]
  isLoading: boolean
  error: string | null
} {
  const [categories, setCategories] = useState<TemplateCategory[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false

    async function load() {
      try {
        const res = await listTemplates()
        if (!cancelled) {
          setCategories(res.categories)
          setIsLoading(false)
        }
      } catch (err) {
        if (!cancelled) {
          setError(String(err))
          setIsLoading(false)
        }
      }
    }

    void load()
    return () => { cancelled = true }
  }, [])

  const allTemplates = categories.flatMap(c => c.templates)

  return { categories, allTemplates, isLoading, error }
}
