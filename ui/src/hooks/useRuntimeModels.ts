import { useEffect, useState } from 'react'
import { listRuntimeModels } from '../data'

export function useRuntimeModels(runtime: string): {
  runtimeModels: string[]
  runtimeModelsError: string | null
} {
  const [runtimeModels, setRuntimeModels] = useState<string[]>([])
  const [runtimeModelsError, setRuntimeModelsError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setRuntimeModels([])
    setRuntimeModelsError(null)

    async function refreshRuntimeModels() {
      try {
        const nextModels = await listRuntimeModels(runtime)
        if (!cancelled) {
          setRuntimeModels(nextModels)
          setRuntimeModelsError(null)
        }
      } catch (err) {
        if (!cancelled) {
          setRuntimeModels([])
          setRuntimeModelsError(String(err))
        }
      }
    }

    void refreshRuntimeModels()

    return () => {
      cancelled = true
    }
  }, [runtime])

  return { runtimeModels, runtimeModelsError }
}
