import { useMemo, useState } from 'react'
import {
  ArrowRight,
  Check,
  ChevronDown,
  Expand,
  MessageSquareText,
  PencilLine,
  Plus,
  Sparkles,
  X,
} from 'lucide-react'
import './WorkspaceWindowPrototype.css'

type Step = {
  id: number
  status: 'complete' | 'current'
  title: string
  subtitle: string
}

type Section = {
  id: number
  title: string
  done: boolean
}

type AgentMessage = {
  id: string
  speaker: string
  body: string
}

const steps: Step[] = [
  { id: 1, status: 'complete', title: 'Agent Configuration', subtitle: 'Ops Specialist' },
  { id: 2, status: 'complete', title: 'Knowledge Sources', subtitle: 'Runbooks, Logs, QA SOP' },
  { id: 3, status: 'complete', title: 'Integration Settings', subtitle: 'Chorus connected' },
  { id: 4, status: 'current', title: 'Automation Rules', subtitle: 'Thread routing' },
]

const sections: Section[] = [
  { id: 1, title: 'General Settings', done: true },
  { id: 2, title: 'Response Templates', done: true },
  { id: 3, title: 'Thread Triage', done: false },
  { id: 4, title: 'Escalation Rules', done: false },
  { id: 5, title: 'Follow-up Scheduling', done: false },
  { id: 6, title: 'Quality Checks', done: false },
]

const quickActions = [
  { id: 'reply', label: 'Draft a reply', icon: PencilLine },
  { id: 'summary', label: 'Summarize thread', icon: Sparkles },
]

const keywordSets = [
  ['handoff', 'blocked', 'ship', 'review', 'regression'],
  ['dm', 'thread', 'agent', 'task', 'workspace'],
  ['lifecycle', 'resume', 'bridge', 'wake', 'failure'],
]

const sampleReplies: Record<string, AgentMessage[]> = {
  default: [
    {
      id: 'a',
      speaker: 'Ops Specialist',
      body: 'This rule watches collaboration signals across channels, DMs, and thread replies before deciding whether to route, summarize, or escalate.',
    },
  ],
  reply: [
    {
      id: 'b',
      speaker: 'Ops Specialist',
      body: 'Draft: “I can route this thread to runtime-lab and attach the latest verification context if you want the agent team to pick it up.”',
    },
  ],
  summary: [
    {
      id: 'c',
      speaker: 'Ops Specialist',
      body: 'Summary: Bridge wakeups are stable, thread traffic is concentrated in runtime-lab, and the release room is still waiting on follow-up verification.',
    },
  ],
}

export function WorkspaceWindowPrototype() {
  const [activeAction, setActiveAction] = useState<'default' | 'reply' | 'summary'>('default')
  const [prompt, setPrompt] = useState('Try: "Route blocked bridge issues to #runtime-lab and summarize related DM context."')

  const keywords = useMemo(
    () => keywordSets[activeAction === 'default' ? 0 : activeAction === 'reply' ? 1 : 2],
    [activeAction]
  )

  const messages = sampleReplies[activeAction]

  return (
    <main className="workbench">
      <div className="workbench__grid" aria-hidden="true" />
      <section className="workbench-window">
        <header className="workbench-window__chrome">
          <div className="workbench-window__traffic" aria-hidden="true">
            <span />
            <span />
            <span />
          </div>
        </header>

        <nav className="workbench-steps" aria-label="Automation steps">
          <button className="workbench-steps__close" type="button" aria-label="Close">
            <X size={18} />
          </button>

          {steps.map((step) => (
            <button key={step.id} className={`workbench-step workbench-step--${step.status}`} type="button">
              <div className="workbench-step__meta">
                <span>{step.status === 'complete' ? 'Complete' : 'Current'}</span>
                <strong>
                  {step.id}. {step.title}
                </strong>
                <small>{step.subtitle}</small>
              </div>
              {step.status === 'complete' ? <Check size={18} /> : <ChevronDown size={18} />}
            </button>
          ))}
        </nav>

        <div className="workbench-layout">
          <aside className="workbench-sidebar">
            <div className="workbench-sidebar__intro">
              <h1>Setup Thread Triage</h1>
              <p>
                Define how Chorus should classify collaboration traffic, escalate risky conversations,
                and route context to the right agent workspace.
              </p>
            </div>

            <div className="workbench-sidebar__sections">
              {sections.map((section) => (
                <button
                  key={section.id}
                  type="button"
                  className={`workbench-section${section.id === 3 ? ' is-active' : ''}`}
                >
                  <span className={`workbench-section__marker${section.done ? ' is-done' : ''}`}>
                    {section.done ? <Check size={14} /> : section.id}
                  </span>
                  <span>{section.title}</span>
                </button>
              ))}
            </div>

            <div className="workbench-progress">
              <div className="workbench-progress__bar">
                <span style={{ width: '58%' }} />
              </div>
              <strong>58% configured</strong>
            </div>
          </aside>

          <section className="workbench-main">
            <div className="workbench-card">
              <div className="workbench-card__label">Rule Name</div>
              <div className="workbench-input">Thread Routing - Runtime Signals</div>
              <div className="workbench-grid-two">
                <div>
                  <div className="workbench-card__label">Route Messages Via</div>
                  <div className="workbench-select">
                    Channel + DM graph
                    <ChevronDown size={16} />
                  </div>
                </div>
                <div>
                  <div className="workbench-card__label">Confidence Threshold</div>
                  <div className="workbench-select">
                    High (85%+)
                    <ChevronDown size={16} />
                  </div>
                </div>
              </div>
            </div>

            <div className="workbench-card">
              <div className="workbench-card__label">Agent Autonomy Level</div>
              <div className="workbench-options">
                <label className="workbench-option">
                  <input type="radio" name="autonomy" />
                  <span>
                    <strong>Review each decision</strong>
                    <small>Agent proposes routing, human confirms before action.</small>
                  </span>
                </label>
                <label className="workbench-option">
                  <input type="radio" name="autonomy" />
                  <span>
                    <strong>Auto-route if confident</strong>
                    <small>Messages above threshold move immediately, uncertain ones are flagged.</small>
                  </span>
                </label>
                <label className="workbench-option">
                  <input type="radio" name="autonomy" defaultChecked />
                  <span>
                    <strong>Escalate complex threads</strong>
                    <small>Threads touching failures, blockers, or release decisions get routed to humans.</small>
                  </span>
                </label>
              </div>
            </div>

            <div className="workbench-card">
              <div className="workbench-card__label">Match Keywords and Intents</div>
              <div className="workbench-tags">
                {keywords.map((keyword) => (
                  <span key={keyword} className="workbench-tag">
                    {keyword}
                    <X size={12} />
                  </span>
                ))}
                <button type="button" className="workbench-tag workbench-tag--add">
                  <Plus size={12} />
                </button>
              </div>
              <p className="workbench-card__hint">
                The agent uses semantic understanding beyond exact matches. These tags help it weight
                routing decisions toward workflow-critical conversations.
              </p>
            </div>

            <div className="workbench-card workbench-card--footer">
              <div className="workbench-grid-three">
                <div>
                  <div className="workbench-card__label">Frequency</div>
                  <div className="workbench-select">
                    Continuous
                    <ChevronDown size={16} />
                  </div>
                </div>
                <div>
                  <div className="workbench-card__label">Hours</div>
                  <div className="workbench-select">
                    24 / 7
                    <ChevronDown size={16} />
                  </div>
                </div>
                <div>
                  <div className="workbench-card__label">Scope</div>
                  <div className="workbench-select">
                    All workspaces
                    <ChevronDown size={16} />
                  </div>
                </div>
              </div>
            </div>
          </section>

          <aside className="workbench-preview">
            <div className="workbench-preview__header">
              <div className="workbench-preview__identity">
                <span className="workbench-preview__avatar" aria-hidden="true" />
                <strong>Ops Specialist (Preview)</strong>
              </div>
              <div className="workbench-preview__actions">
                <Expand size={18} />
                <X size={18} />
              </div>
            </div>

            <div className="workbench-preview__body">
              {messages.map((message) => (
                <article key={message.id} className="workbench-preview__message">
                  <strong>{message.speaker}</strong>
                  <p>{message.body}</p>
                </article>
              ))}

              <div className="workbench-preview__quick">
                {quickActions.map((action) => {
                  const Icon = action.icon
                  return (
                    <button
                      key={action.id}
                      type="button"
                      className={`workbench-preview__quick-action${activeAction === action.id ? ' is-active' : ''}`}
                      onClick={() => setActiveAction(action.id as 'reply' | 'summary')}
                    >
                      <Icon size={16} />
                      {action.label}
                    </button>
                  )
                })}
              </div>
            </div>

            <label className="workbench-preview__composer">
              <textarea
                value={prompt}
                onChange={(event) => setPrompt(event.target.value)}
                aria-label="Preview prompt"
              />
              <div className="workbench-preview__composer-footer">
                <button type="button" className="workbench-model">
                  <MessageSquareText size={14} />
                  Chorus Agent
                </button>
                <button type="button" className="workbench-send" aria-label="Send prompt">
                  <ArrowRight size={18} />
                </button>
              </div>
            </label>
          </aside>
        </div>
      </section>
    </main>
  )
}
