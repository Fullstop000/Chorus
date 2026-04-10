import { useState, useCallback } from 'react'
import { LoaderCircle, RefreshCw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { launchTrio } from '../../data/templates'
import type { AgentTemplate } from '../../hooks/useTemplates'
import './LaunchTrio.css'

function pickRandom3(templates: AgentTemplate[]): AgentTemplate[] {
  if (templates.length <= 3) return templates
  const shuffled = [...templates].sort(() => Math.random() - 0.5)
  return shuffled.slice(0, 3)
}

interface Props {
  allTemplates: AgentTemplate[]
  onLaunched: (channelId: string) => void
}

export function LaunchTrio({ allTemplates, onLaunched }: Props) {
  const [isLaunching, setIsLaunching] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [trio, setTrio] = useState<AgentTemplate[] | null>(null)

  const trioTemplates = trio ?? pickRandom3(allTemplates)

  const handleShuffle = useCallback(() => {
    setTrio(pickRandom3(allTemplates))
  }, [allTemplates])

  if (allTemplates.length < 3) {
    return null
  }

  async function handleLaunch() {
    setIsLaunching(true)
    setError(null)
    try {
      const ids = trioTemplates.map(t => t.id)
      const res = await launchTrio(ids)
      if (res.errors && res.errors.length > 0) {
        const failedNames = res.errors.map(e => e.template_id).join(', ')
        setError(`Some agents failed to create: ${failedNames}`)
      }
      onLaunched(res.channel_id)
    } catch (e) {
      setError(String(e))
    } finally {
      setIsLaunching(false)
    }
  }

  return (
    <div className="launch-trio">
      <span className="launch-trio-kicker">Launch Trio</span>

      <div className="launch-trio-cards">
        {trioTemplates.map(t => (
          <div key={t.id} className="launch-trio-card">
            <span className="launch-trio-card-emoji">{t.emoji ?? '🤖'}</span>
            <span className="launch-trio-card-name">{t.name}</span>
            <span className="launch-trio-card-role">{t.category}</span>
          </div>
        ))}
      </div>

      <div className="launch-trio-actions">
        <button
          className="launch-trio-shuffle"
          onClick={handleShuffle}
          disabled={isLaunching}
          type="button"
        >
          <RefreshCw size={12} />
          Shuffle
        </button>
        <Button
          size="sm"
          className="launch-trio-btn"
          onClick={handleLaunch}
          disabled={isLaunching}
        >
          {isLaunching ? (
            <>
              <LoaderCircle size={11} className="launch-trio-spinner" />
              Launching...
            </>
          ) : (
            'Launch All 3'
          )}
        </Button>
      </div>

      {error && <p className="launch-trio-error">{error}</p>}
    </div>
  )
}
