import { useEffect, useState } from 'react'
import { listRuntimeModels } from '../data'

export function useRuntimeModels(runtime: string): {
  runtimeModels: string[]
  runtimeModelsError: string | null
  isLoading: boolean
} {
  const [runtimeModels, setRuntimeModels] = useState<string[]>([])
  const [runtimeModelsError, setRuntimeModelsError] = useState<string | null>(null)
  const [isLoading, setIsLoading] = useState(false)

  useEffect(() => {
    let cancelled = false
    setRuntimeModels([])
    setRuntimeModelsError(null)
    setIsLoading(true)

    async function refreshRuntimeModels() {
      try {
        const nextModels = await listRuntimeModels(runtime)
        if (!cancelled) {
          setRuntimeModels(nextModels)
          setRuntimeModelsError(null)
          setIsLoading(false)
        }
      } catch (err) {
        if (!cancelled) {
          setRuntimeModels([])
          setRuntimeModelsError(String(err))
          setIsLoading(false)
        }
      }
    }

    void refreshRuntimeModels()

    return () => {
      cancelled = true
    }
  }, [runtime])

  return { runtimeModels, runtimeModelsError, isLoading }
}
