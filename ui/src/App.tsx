import { AppProvider } from './store'
import { Sidebar } from './components/Sidebar'
import { MainPanel } from './components/MainPanel'

export default function App() {
  return (
    <AppProvider>
      <div className="app-shell">
        <Sidebar />
        <MainPanel />
      </div>
    </AppProvider>
  )
}
