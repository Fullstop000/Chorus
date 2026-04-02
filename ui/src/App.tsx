import { QueryClientProvider } from '@tanstack/react-query'
import { queryClient } from './lib/utils'
import { AppProvider } from './store'
import { Sidebar } from './pages/Sidebar'
import { MainPanel } from './pages/MainPanel'

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <AppProvider>
        <div className="app-shell">
          <Sidebar />
          <MainPanel />
        </div>
      </AppProvider>
    </QueryClientProvider>
  )
}
