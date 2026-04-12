import { QueryClient } from '@tanstack/react-query'
import { pushErrorToast } from '@/store/uiStore'

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: 1,
    },
    mutations: {
      onError: (err) => pushErrorToast(err),
    },
  },
})
