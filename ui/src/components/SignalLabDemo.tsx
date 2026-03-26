import { useState } from 'react'
import {
  Activity,
  Bot,
  BrainCircuit,
  CheckCircle2,
  Command,
  Cpu,
  GitBranch,
  Layers3,
  MessageSquareText,
  Radio,
  Send,
  Sparkles,
  TerminalSquare,
  Workflow,
} from 'lucide-react'
import './SignalLabDemo.css'

type DemoMessage = {
  speaker: string
  kind: 'agent' | 'human'
  time: string
  body: string
}

type DemoChannel = {
  name: string
  meta: string
  unread: number
  nodeId: string
  presence: string
  anomalyCount: number
  summary: string
  metrics: Array<{ label: string; value: string }>
  activity: Array<{ title: string; detail: string; icon: 'check' | 'workflow' | 'terminal' }>
  draft: string
  messages: DemoMessage[]
}

const initialChannels: DemoChannel[] = [
  {
    name: 'orchestra',
    meta: '12 live',
    unread: 4,
    nodeId: 'NODE-01',
    presence: '12 active nodes',
    anomalyCount: 0,
    summary:
      'The coordination room for broad planning. Human and agent traffic is mixed, with quick routing decisions and cross-channel callouts.',
    metrics: [
      { label: 'Active threads', value: '11' },
      { label: 'Agent quorum', value: '6' },
      { label: 'Queued asks', value: '24' },
    ],
    activity: [
      { title: 'Runbook updated', detail: 'release checklist pinned for all operators', icon: 'check' },
      { title: 'New routing request', detail: 'design audit forwarded to claude-watch', icon: 'workflow' },
      { title: 'Bridge session linked', detail: 'codex-ops joined shared workspace thread', icon: 'terminal' },
    ],
    draft: '@claude-watch summarize the open workstreams before standup',
    messages: [
      {
        speaker: 'maya',
        kind: 'human',
        time: '13:58:10',
        body: 'We need a fast visibility pass across design, runtime, and release prep before the afternoon review.',
      },
      {
        speaker: 'claude-watch',
        kind: 'agent',
        time: '13:59:04',
        body: 'Design review is waiting on shell direction. Runtime fixes are scoped. Release prep is blocked on browser verification.',
      },
    ],
  },
  {
    name: 'runtime-lab',
    meta: '3 anomalies',
    unread: 2,
    nodeId: 'NODE-02',
    presence: '3 active nodes',
    anomalyCount: 1,
    summary:
      'The debugging room for agent lifecycle and bridge behavior. This feed should feel like a live machine: wakeups, probes, and bounded fixes.',
    metrics: [
      { label: 'Queue stability', value: '94%' },
      { label: 'Wake cycles', value: '18' },
      { label: 'Threads linked', value: '7' },
    ],
    activity: [
      { title: 'Wake notification delivered', detail: 'bridge-7 acknowledged channel handoff', icon: 'check' },
      { title: 'Task state mutated', detail: 'release-gate moved to guarded verification', icon: 'workflow' },
      { title: 'CLI session resumed', detail: 'codex thread linked back into shared workspace', icon: 'terminal' },
    ],
    draft: '@codex-ops run post-fix verification on runtime-lab',
    messages: [
      {
        speaker: 'codex-ops',
        kind: 'agent',
        time: '14:03:11',
        body: 'Patched the bridge wakeup path. Holding for browser verification before reopening the release gate.',
      },
      {
        speaker: 'maya',
        kind: 'human',
        time: '14:03:48',
        body: 'Keep the fix scoped. If task board state mutates on reconnect, capture it as a separate finding.',
      },
      {
        speaker: 'claude-watch',
        kind: 'agent',
        time: '14:04:02',
        body: 'Observed one stale presence pulse in the activity rail. No data loss signal yet. Continuing to monitor.',
      },
    ],
  },
  {
    name: 'shiproom',
    meta: 'release gate',
    unread: 1,
    nodeId: 'NODE-03',
    presence: '5 active nodes',
    anomalyCount: 2,
    summary:
      'The release room is for go/no-go decisions, evidence snapshots, and tightening risk communication before work lands.',
    metrics: [
      { label: 'Gate status', value: 'HOLD' },
      { label: 'Fixes ready', value: '3' },
      { label: 'QA gaps', value: '2' },
    ],
    activity: [
      { title: 'QA follow-up requested', detail: 'messaging regression cases need tighter coverage', icon: 'workflow' },
      { title: 'Preview snapshot captured', detail: 'dark and light shell evidence attached', icon: 'terminal' },
      { title: 'Release gate unresolved', detail: 'awaiting human decision on fix pass', icon: 'check' },
    ],
    draft: '@codex-ops prepare the release risk summary for human review',
    messages: [
      {
        speaker: 'bridge-7',
        kind: 'agent',
        time: '14:01:09',
        body: 'Current release gate is blocked on QA evidence. No safe path to clear without browser confirmation.',
      },
      {
        speaker: 'maya',
        kind: 'human',
        time: '14:02:31',
        body: 'Keep the release room factual. I only want severity, exact cases, and the next decision point.',
      },
    ],
  },
]

const agents = [
  { name: 'codex-ops', role: 'Execution node', status: 'routing fix pass', tone: 'cyan' },
  { name: 'claude-watch', role: 'Reasoning node', status: 'triaging messages', tone: 'violet' },
  { name: 'bridge-7', role: 'Lifecycle probe', status: 'idle but subscribed', tone: 'lime' },
]

function iconForActivity(kind: DemoChannel['activity'][number]['icon']) {
  if (kind === 'workflow') return <Workflow aria-hidden="true" size={14} />
  if (kind === 'terminal') return <TerminalSquare aria-hidden="true" size={14} />
  return <CheckCircle2 aria-hidden="true" size={14} />
}

function currentTimeLabel() {
  return new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
}

function anomalyLabel(count: number) {
  return count === 1 ? 'anomaly' : 'anomalies'
}

export function SignalLabDemo() {
  const params = new URLSearchParams(window.location.search)
  const theme = params.get('theme') === 'light' ? 'light' : 'dark'
  const [selectedChannelName, setSelectedChannelName] = useState('runtime-lab')
  const [channels, setChannels] = useState(initialChannels)

  const selectedChannel =
    channels.find((channel) => channel.name === selectedChannelName) ?? channels[0]

  function handleSelectChannel(name: string) {
    setSelectedChannelName(name)
    setChannels((current) =>
      current.map((channel) =>
        channel.name === name ? { ...channel, unread: 0 } : channel
      )
    )
  }

  function handleDraftChange(value: string) {
    setChannels((current) =>
      current.map((channel) =>
        channel.name === selectedChannel.name ? { ...channel, draft: value } : channel
      )
    )
  }

  function handleSendMessage() {
    const trimmed = selectedChannel.draft.trim()
    if (!trimmed) return

    setChannels((current) =>
      current.map((channel) =>
        channel.name === selectedChannel.name
          ? {
              ...channel,
              unread: 0,
              draft: '',
              messages: [
                ...channel.messages,
                {
                  speaker: 'maya',
                  kind: 'human',
                  time: currentTimeLabel(),
                  body: trimmed,
                },
              ],
            }
          : channel
      )
    )
  }

  return (
    <main className={`signal-lab signal-lab--${theme}`}>
      <div className="signal-lab__backdrop" aria-hidden="true" />
      <div className="signal-lab__scanlines" aria-hidden="true" />
      <aside className="signal-sidebar">
        <div className="signal-sidebar__brand">
          <div className="signal-sidebar__brand-mark">
            <BrainCircuit aria-hidden="true" size={18} />
          </div>
          <div>
            <p className="signal-kicker">Experimental FUI preview</p>
            <h1>Chorus Signal Lab</h1>
            <p className="signal-bootline">boot seq 07 :: shared memory online</p>
          </div>
        </div>

        <section className="signal-sidebar__section">
          <div className="signal-sidebar__section-header">
            <span>Channels</span>
            <button type="button" aria-label="Create channel">
              <Layers3 aria-hidden="true" size={14} />
            </button>
          </div>
          <div className="signal-stack">
            {channels.map((channel, index) => (
              <button
                key={channel.name}
                type="button"
                className={`signal-nav-card${channel.name === selectedChannel.name ? ' is-active' : ''}`}
                onClick={() => handleSelectChannel(channel.name)}
              >
                <span className="signal-nav-card__glyph">#{index + 1}</span>
                <span className="signal-nav-card__body">
                  <strong>{channel.name}</strong>
                  <small>{channel.meta}</small>
                </span>
                <span className="signal-nav-card__node">{channel.nodeId}</span>
                {channel.unread > 0 && <span className="signal-nav-card__count">{channel.unread}</span>}
              </button>
            ))}
          </div>
        </section>

        <section className="signal-sidebar__section">
          <div className="signal-sidebar__section-header">
            <span>Agents</span>
            <button type="button" aria-label="Create agent">
              <Bot aria-hidden="true" size={14} />
            </button>
          </div>
          <div className="signal-stack">
            {agents.map((agent, index) => (
              <button
                key={agent.name}
                type="button"
                className={`signal-agent-card tone-${agent.tone}${index === 0 ? ' is-active' : ''}`}
              >
                <span className="signal-agent-card__avatar" aria-hidden="true">
                  <Cpu size={14} />
                </span>
                <span className="signal-agent-card__body">
                  <strong>{agent.name}</strong>
                  <small>{agent.role}</small>
                </span>
                <span className="signal-agent-card__status">{agent.status}</span>
              </button>
            ))}
          </div>
        </section>

        <div className="signal-sidebar__footer">
          <div>
            <p className="signal-kicker">Operator</p>
            <strong>maya</strong>
          </div>
          <button type="button" className="signal-ghost-button">
            <Command aria-hidden="true" size={14} />
            Console
          </button>
        </div>
      </aside>

      <section className="signal-main">
        <header className="signal-topbar">
          <div>
            <p className="signal-kicker">Live room</p>
            <h2>#{selectedChannel.name}</h2>
            <p className="signal-terminal-line">bus {selectedChannel.nodeId} :: relay stable :: human+agent mixed traffic</p>
          </div>
          <div className="signal-topbar__meta">
            <a
              className={`signal-pill signal-pill--link${theme === 'dark' ? ' is-active' : ''}`}
              href="?demo=signal-lab"
            >
              Dark
            </a>
            <a
              className={`signal-pill signal-pill--link${theme === 'light' ? ' is-active' : ''}`}
              href="?demo=signal-lab&theme=light"
            >
              Light
            </a>
            <span className="signal-pill">
              <Radio aria-hidden="true" size={12} />
              {selectedChannel.presence}
            </span>
            <span className="signal-pill signal-pill--warning">
              <Sparkles aria-hidden="true" size={12} />
              {selectedChannel.anomalyCount} {anomalyLabel(selectedChannel.anomalyCount)}
            </span>
          </div>
        </header>

        <div className="signal-dashboard">
          <section className="signal-panel signal-panel--hero">
            <div className="signal-hero">
              <div>
                <p className="signal-kicker">System narrative</p>
                <h3>Agent collaboration as a visible machine</h3>
                <p className="signal-hero__copy">{selectedChannel.summary}</p>
              </div>
              <div className="signal-metrics" aria-label="System metrics">
                {selectedChannel.metrics.map((metric) => (
                  <article key={metric.label} className="signal-metric-card">
                    <span>{metric.label}</span>
                    <strong className="signal-segmented">{metric.value}</strong>
                  </article>
                ))}
              </div>
            </div>
          </section>

          <section className="signal-panel signal-panel--chat">
            <div className="signal-panel__header">
              <span>
                <MessageSquareText aria-hidden="true" size={14} />
                Conversation feed
              </span>
              <span className="signal-panel__meta">{selectedChannel.messages.length} messages</span>
            </div>
            <div className="signal-feed" aria-live="polite">
              {selectedChannel.messages.map((message) => (
                <article
                  key={`${selectedChannel.name}-${message.speaker}-${message.time}-${message.body}`}
                  className={`signal-message signal-message--${message.kind}`}
                >
                  <div className="signal-message__avatar" aria-hidden="true">
                    {message.kind === 'agent' ? <Cpu size={14} /> : <GitBranch size={14} />}
                  </div>
                  <div className="signal-message__body">
                    <div className="signal-message__meta">
                      <strong>{message.speaker}</strong>
                      <span>{message.time}</span>
                    </div>
                    <p>{message.body}</p>
                  </div>
                </article>
              ))}
            </div>
            <label className="signal-composer">
              <span className="signal-composer__label">Command input</span>
              <div className="signal-composer__row">
                <input
                  type="text"
                  value={selectedChannel.draft}
                  onChange={(event) => handleDraftChange(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') {
                      event.preventDefault()
                      handleSendMessage()
                    }
                  }}
                  aria-label={`Message ${selectedChannel.name}`}
                />
                <button type="button" aria-label="Send command" onClick={handleSendMessage}>
                  <Send aria-hidden="true" size={14} />
                </button>
              </div>
            </label>
          </section>

          <section className="signal-panel signal-panel--rail">
            <div className="signal-panel__header">
              <span>
                <Activity aria-hidden="true" size={14} />
                Activity rail
              </span>
              <span className="signal-panel__meta">selected channel</span>
            </div>
            <div className="signal-events">
              {selectedChannel.activity.map((event) => (
                <div key={event.title} className="signal-event">
                  {iconForActivity(event.icon)}
                  <div>
                    <strong>{event.title}</strong>
                    <span>{event.detail}</span>
                  </div>
                </div>
              ))}
            </div>
          </section>
        </div>
      </section>
    </main>
  )
}
