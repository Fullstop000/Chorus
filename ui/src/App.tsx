import { QueryClientProvider } from '@tanstack/react-query'
import { queryClient } from './queryClient'
import { AppProvider } from './store'
import { Sidebar } from './components/Sidebar'
import { MainPanel } from './components/MainPanel'

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
