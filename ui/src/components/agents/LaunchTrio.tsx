import { useState } from 'react'
import { LoaderCircle } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { launchTrio } from '../../data/templates'
import type { AgentTemplate } from '../../hooks/useTemplates'
import './LaunchTrio.css'

const DEFAULT_TRIO_IDS = [
  'engineering/backend-architect',
  'engineering/code-reviewer',
  'product/behavioral-nudge-engine',
]

interface Props {
  allTemplates: AgentTemplate[]
  onLaunched: (channelId: string) => void
}

export function LaunchTrio({ allTemplates, onLaunched }: Props) {
  const [isLaunching, setIsLaunching] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Only show if all trio templates exist in the loaded set.
  const trioTemplates = DEFAULT_TRIO_IDS
    .map(id => allTemplates.find(t => t.id === id))
    .filter((t): t is AgentTemplate => t !== undefined)

  if (trioTemplates.length !== DEFAULT_TRIO_IDS.length) {
    return null
  }

  async function handleLaunch() {
    setIsLaunching(true)
    setError(null)
    try {
      const res = await launchTrio(DEFAULT_TRIO_IDS)
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

  const agentLine = trioTemplates
    .map(t => `${t.emoji ?? '🤖'} ${t.name}`)
    .join('  ·  ')

  return (
    <div className="launch-trio">
      <div className="launch-trio-header">
        <span className="launch-trio-label">Launch Trio</span>
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
      <div className="launch-trio-agents">{agentLine}</div>
      {error && <p className="launch-trio-error">{error}</p>}
    </div>
  )
}
