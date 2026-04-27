import { useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Users } from 'lucide-react'
import './MentionTextarea.css'

export interface MentionMember {
  name: string
  displayName?: string
  type: 'agent' | 'human' | 'team'
}

interface Props {
  value: string
  onChange: (value: string) => void
  onEnter?: () => void
  placeholder?: string
  disabled?: boolean
  className?: string
  rows?: number
  members: MentionMember[]
}

/** Detect @mention query at cursor position */
function getMentionQuery(value: string, cursor: number): string | null {
  const before = value.slice(0, cursor)
  const match = before.match(/@(\w*)$/)
  return match ? match[1] : null
}

function memberInitial(name: string) {
  return name[0]?.toUpperCase() ?? '?'
}

function memberColor(name: string): string {
  const colors = ['#C0392B', '#2980B9', '#27AE60', '#8E44AD', '#D35400', '#16A085', '#2C3E50']
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

export function MentionTextarea({
  value,
  onChange,
  onEnter,
  placeholder,
  disabled,
  className,
  rows = 1,
  members,
}: Props) {
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([])
  const [mentionQuery, setMentionQuery] = useState<string | null>(null)
  const [highlightIdx, setHighlightIdx] = useState(0)

  const suggestions =
    mentionQuery !== null
      ? members.filter((m) => {
          const q = mentionQuery.toLowerCase()
          return (
            m.name.toLowerCase().startsWith(q) ||
            (m.displayName?.toLowerCase().startsWith(q) ?? false)
          )
        })
      : []

  function closeMention() {
    setMentionQuery(null)
    setHighlightIdx(0)
  }

  useEffect(() => {
    if (suggestions.length === 0) {
      itemRefs.current = []
      return
    }
    const activeItem = itemRefs.current[highlightIdx]
    activeItem?.scrollIntoView({ block: 'nearest' })
  }, [highlightIdx, suggestions.length])

  function insertMention(name: string) {
    const ta = textareaRef.current
    if (!ta) return
    const cursor = ta.selectionStart
    const before = value.slice(0, cursor)
    const after = value.slice(cursor)
    const newBefore = before.replace(/@\w*$/, `@${name} `)
    onChange(newBefore + after)
    closeMention()
    const newCursor = newBefore.length
    setTimeout(() => {
      ta.focus()
      ta.setSelectionRange(newCursor, newCursor)
    }, 0)
  }

  function handleChange(e: React.ChangeEvent<HTMLTextAreaElement>) {
    const val = e.target.value
    onChange(val)
    const cursor = e.target.selectionStart
    const query = getMentionQuery(val, cursor)
    setMentionQuery(query)
    setHighlightIdx(0)
  }

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (suggestions.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setHighlightIdx((i) => (i + 1) % suggestions.length)
        return
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault()
        setHighlightIdx((i) => (i - 1 + suggestions.length) % suggestions.length)
        return
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault()
        insertMention(suggestions[highlightIdx].name)
        return
      }
      if (e.key === 'Escape') {
        e.preventDefault()
        closeMention()
        return
      }
    }

    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      onEnter?.()
    }
  }

  return (
    <div className="mention-textarea-wrap">
      {suggestions.length > 0 && (
        <div className="mention-popup">
          {suggestions.map((m, i) => (
            <button
              key={m.name}
              ref={(node) => {
                itemRefs.current[i] = node
              }}
              className={`mention-popup-item${i === highlightIdx ? ' highlighted' : ''}`}
              onMouseDown={(e) => {
                e.preventDefault() // Don't blur textarea
                insertMention(m.name)
              }}
              onMouseEnter={() => setHighlightIdx(i)}
            >
              <span
                className={`mention-popup-avatar${m.type === 'team' ? ' team' : ''}`}
                style={m.type === 'team' ? undefined : { background: memberColor(m.name) }}
              >
                {m.type === 'team' ? <Users size={12} /> : memberInitial(m.displayName ?? m.name)}
              </span>
              <span className="mention-popup-name">@{m.displayName ?? m.name}</span>
              {m.type === 'agent' && <span className="mention-popup-badge">BOT</span>}
              {m.type === 'team' && <span className="mention-popup-badge">TEAM</span>}
            </button>
          ))}
        </div>
      )}
      <textarea
        ref={textareaRef}
        className={className}
        placeholder={placeholder}
        value={value}
        onChange={handleChange}
        onKeyDown={handleKeyDown}
        disabled={disabled}
        rows={rows}
      />
    </div>
  )
}
