import { useMemo, useState } from 'react'
import {
  ArrowRight,
  Check,
  Hash,
  MessageSquareText,
  Paperclip,
  Search,
  Sparkles,
  Users,
  X,
} from 'lucide-react'
import './ChorusMessagingPrototype.css'

type Channel = {
  id: string
  name: string
  subtitle: string
  unread: number
  members: number
  tags: string[]
}

type Message = {
  id: string
  author: string
  role: 'human' | 'agent'
  time: string
  body: string
}

const channels: Channel[] = [
  {
    id: 'runtime-lab',
    name: 'runtime-lab',
    subtitle: 'bridge wakeups, resumes, lifecycle',
    unread: 3,
    members: 6,
    tags: ['bridge', 'lifecycle', 'critical'],
  },
  {
    id: 'shiproom',
    name: 'shiproom',
    subtitle: 'release gate and follow-up QA',
    unread: 1,
    members: 4,
    tags: ['release', 'evidence', 'hold'],
  },
  {
    id: 'orchestra',
    name: 'orchestra',
    subtitle: 'cross-team routing and planning',
    unread: 0,
    members: 12,
    tags: ['triage', 'design', 'ops'],
  },
]

const conversationMap: Record<string, Message[]> = {
  'runtime-lab': [
    {
      id: 'm1',
      author: 'codex-ops',
      role: 'agent',
      time: '14:03',
      body: 'Bridge wakeup path is patched. The remaining risk is whether thread state rehydrates correctly after resume.',
    },
    {
      id: 'm2',
      author: 'maya',
      role: 'human',
      time: '14:05',
      body: 'Keep the next pass narrow. I want browser evidence for thread continuity before we clear the room.',
    },
    {
      id: 'm3',
      author: 'claude-watch',
      role: 'agent',
      time: '14:07',
      body: 'No dropped context observed in the last three wake cycles. One stale presence pulse remains, but it looks cosmetic.',
    },
  ],
  shiproom: [
    {
      id: 'm4',
      author: 'bridge-7',
      role: 'agent',
      time: '13:58',
      body: 'Release gate remains blocked on post-fix browser verification and an explicit human decision.',
    },
    {
      id: 'm5',
      author: 'maya',
      role: 'human',
      time: '14:00',
      body: 'Summaries only in this room. Severity, exact failing workflow, and next action.',
    },
  ],
  orchestra: [
    {
      id: 'm6',
      author: 'maya',
      role: 'human',
      time: '13:41',
      body: 'We need the new visual language to feel native to Chorus, not like a pasted dashboard skin.',
    },
    {
      id: 'm7',
      author: 'claude-watch',
      role: 'agent',
      time: '13:44',
      body: 'Recommendation: validate the windowed style first on messaging. If that works, extend it to tasks, profiles, and activity.',
    },
  ],
}

const threadNotes = [
  'Thread contains agent replies, human decisions, and system-risk language.',
  'Suggested action: route follow-up to runtime-lab and attach verification context.',
  'Last cross-reference: shiproom release gate note at 14:00.',
]

export function ChorusMessagingPrototype() {
  const [activeChannelId, setActiveChannelId] = useState('runtime-lab')
  const [draft, setDraft] = useState('Draft a routing note or ask an agent to summarize this room.')

  const activeChannel = channels.find((channel) => channel.id === activeChannelId) ?? channels[0]
  const messages = useMemo(() => conversationMap[activeChannelId] ?? [], [activeChannelId])

  return (
    <main className="chorus-demo">
      <div className="chorus-demo__grid" aria-hidden="true" />
      <section className="chorus-demo__window">
        <header className="chorus-demo__chrome">
          <div className="chorus-demo__traffic" aria-hidden="true">
            <span />
            <span />
            <span />
          </div>
        </header>

        <div className="chorus-demo__tabs">
          <button className="chorus-demo__tab-close" type="button" aria-label="Close">
            <X size={18} />
          </button>
          <div className="chorus-demo__tab is-active">
            <span>Current</span>
            <strong>1. Chorus Messaging</strong>
            <small>Windowed collaboration panel</small>
          </div>
          <div className="chorus-demo__tab">
            <span>Next</span>
            <strong>2. Thread Detail</strong>
            <small>Context and follow-up routing</small>
          </div>
          <div className="chorus-demo__tab">
            <span>Later</span>
            <strong>3. Activity Rail</strong>
            <small>System events and agent state</small>
          </div>
        </div>

        <div className="chorus-demo__layout">
          <aside className="chorus-demo__channels">
            <div className="chorus-demo__intro">
              <div className="chorus-demo__ascii">[chorus::msg_panel]</div>
              <h1>Chorus Messaging</h1>
              <p>
                A windowed messaging surface in the new light language: calm canvas, precise chrome,
                and enough density for real collaboration.
              </p>
            </div>

            <div className="chorus-demo__section-label">Channels</div>
            <div className="chorus-demo__channel-list">
              {channels.map((channel) => (
                <button
                  key={channel.id}
                  type="button"
                  className={`chorus-demo__channel${channel.id === activeChannelId ? ' is-active' : ''}`}
                  onClick={() => setActiveChannelId(channel.id)}
                >
                  <div className="chorus-demo__channel-header">
                    <span className="chorus-demo__channel-name">
                      <Hash size={14} />
                      {channel.name}
                    </span>
                    {channel.unread > 0 && <span className="chorus-demo__unread">{channel.unread}</span>}
                  </div>
                  <div className="chorus-demo__channel-ascii">:: {channel.members.toString().padStart(2, '0')} nodes online</div>
                  <small>{channel.subtitle}</small>
                </button>
              ))}
            </div>

            <div className="chorus-demo__progress">
              <div className="chorus-demo__progress-bar">
                <span style={{ width: '64%' }} />
              </div>
              <strong>64% shell alignment</strong>
            </div>
          </aside>

          <section className="chorus-demo__main">
            <header className="chorus-demo__header-card">
              <div>
                <div className="chorus-demo__eyebrow">Room</div>
                <h2>#{activeChannel.name}</h2>
                <p>{activeChannel.subtitle}</p>
              </div>
              <div className="chorus-demo__header-meta">
                <span>
                  <Users size={14} />
                  {activeChannel.members} members
                </span>
                <span>
                  <Search size={14} />
                  active context
                </span>
              </div>
            </header>

            <section className="chorus-demo__messages-card">
              <div className="chorus-demo__panel-head">
                <span>
                  <MessageSquareText size={15} />
                  Conversation Feed
                </span>
                <strong>[feed::{messages.length.toString().padStart(2, '0')}]</strong>
              </div>

              <div className="chorus-demo__messages">
                {messages.map((message) => (
                  <article key={message.id} className={`chorus-demo__message chorus-demo__message--${message.role}`}>
                    <div className="chorus-demo__avatar" aria-hidden="true">
                      {message.author.slice(0, 2).toUpperCase()}
                    </div>
                    <div className="chorus-demo__message-body">
                      <div className="chorus-demo__message-meta">
                        <strong>{message.author}</strong>
                        <span>&gt; {message.time}</span>
                      </div>
                      <p>{message.body}</p>
                    </div>
                  </article>
                ))}
              </div>

              <label className="chorus-demo__composer">
                <div className="chorus-demo__composer-ascii">[compose::route_or_reply]</div>
                <textarea
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  aria-label="Message draft"
                />
                <div className="chorus-demo__composer-footer">
                  <div className="chorus-demo__composer-tools">
                    <button type="button" className="chorus-demo__tool">
                      <Paperclip size={14} />
                      Attach
                    </button>
                    <button type="button" className="chorus-demo__tool">
                      <Sparkles size={14} />
                      Summarize
                    </button>
                  </div>
                  <button type="button" className="chorus-demo__send" aria-label="Send message">
                    <ArrowRight size={18} />
                  </button>
                </div>
              </label>
            </section>
          </section>

          <aside className="chorus-demo__thread">
            <div className="chorus-demo__thread-card">
              <div className="chorus-demo__thread-header">
                <div className="chorus-demo__thread-title">
                  <span className="chorus-demo__thread-avatar" aria-hidden="true" />
                  <strong>Thread Context</strong>
                </div>
                <button type="button" className="chorus-demo__ghost" aria-label="Resolve">
                  <Check size={16} />
                </button>
              </div>

              <div className="chorus-demo__thread-body">
                <div className="chorus-demo__ascii">[ctx::{activeChannel.name}]</div>
                <p>
                  This sidecar stays narrow and quiet. It holds routing hints, related room references,
                  and the agent’s next suggested action without overpowering the main conversation.
                </p>

                <div className="chorus-demo__tag-row">
                  {activeChannel.tags.map((tag) => (
                    <span key={tag} className="chorus-demo__tag">
                      {tag}
                    </span>
                  ))}
                </div>

                <div className="chorus-demo__notes">
                  {threadNotes.map((note) => (
                    <div key={note} className="chorus-demo__note">
                      {note}
                    </div>
                  ))}
                </div>
              </div>

              <div className="chorus-demo__thread-footer">
                <button type="button" className="chorus-demo__quick-action">
                  <Sparkles size={14} />
                  Draft a reply
                </button>
                <button type="button" className="chorus-demo__quick-action">
                  <MessageSquareText size={14} />
                  Link related room
                </button>
              </div>
            </div>
          </aside>
        </div>
      </section>
    </main>
  )
}
