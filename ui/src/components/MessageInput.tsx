import { useState, useRef } from 'react'
import { Paperclip, Plus } from 'lucide-react'
import { useApp, useTarget } from '../store'
import { sendMessage, createTasks, uploadFile } from '../api'
import { MentionTextarea } from './MentionTextarea'
import type { MentionMember } from './MentionTextarea'

interface Props {
  onMessageSent?: () => void
}

export function MessageInput({ onMessageSent }: Props) {
  const { currentUser, selectedChannel, serverInfo, agents, teams } = useApp()
  const target = useTarget()
  const [content, setContent] = useState('')
  const [alsoTask, setAlsoTask] = useState(false)
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [pendingFiles, setPendingFiles] = useState<File[]>([])
  const fileInputRef = useRef<HTMLInputElement>(null)

  const members: MentionMember[] = [
    ...agents.map((a) => ({ name: a.name, type: 'agent' as const })),
    ...(serverInfo?.humans ?? []).map((h) => ({ name: h.name, type: 'human' as const })),
    ...teams.map((team) => ({ name: team.name, type: 'team' as const })),
  ]

  // Only protected system channels (e.g. #shared-memory) are read-only.
  const isSystemChannel = !!(selectedChannel && serverInfo?.system_channels?.some(
    (c) => `#${c.name}` === selectedChannel && c.read_only
  ))

  const placeholder = isSystemChannel
    ? `${target} is read-only — agent breadcrumbs only`
    : target
    ? `Message ${target}`
    : 'Select a channel to message'

  async function handleSend() {
    if (!target || !currentUser || (!content.trim() && pendingFiles.length === 0)) return
    setSending(true)
    setError(null)
    try {
      // Upload files first
      const attachmentIds: string[] = []
      for (const file of pendingFiles) {
        const res = await uploadFile(currentUser, file)
        attachmentIds.push(res.id)
      }

      await sendMessage(currentUser, target, content.trim(), attachmentIds, {
        suppressAgentDelivery: alsoTask && !!selectedChannel,
      })

      if (alsoTask && selectedChannel && content.trim()) {
        await createTasks(currentUser, selectedChannel, [content.trim()])
      }

      setContent('')
      setPendingFiles([])
      setAlsoTask(false)
      onMessageSent?.()
    } catch (e) {
      console.error('Send failed:', e)
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSending(false)
    }
  }

  function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? [])
    setError(null)
    setPendingFiles((prev) => [...prev, ...files])
    if (fileInputRef.current) fileInputRef.current.value = ''
  }

  return (
    <div className="message-input-area">
      {error && <div className="error-banner">{error}</div>}
      {pendingFiles.length > 0 && (
        <div className="message-input-files">
          {pendingFiles.map((f, i) => (
            <span key={i} className="file-chip">
              <Paperclip size={12} />
              {f.name}
              <button
                onClick={() => {
                  setError(null)
                  setPendingFiles((prev) => prev.filter((_, j) => j !== i))
                }}
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}
      <div className="message-input-row">
        <button
          className="message-input-btn attach-btn"
          onClick={() => fileInputRef.current?.click()}
          disabled={!target || isSystemChannel}
          title="Attach file"
        >
          <Plus size={16} />
        </button>
        <input
          ref={fileInputRef}
          type="file"
          multiple
          style={{ display: 'none' }}
          onChange={handleFileChange}
        />
        <MentionTextarea
          className="message-input-textarea"
          placeholder={placeholder}
          value={content}
          onChange={(value) => {
            setError(null)
            setContent(value)
          }}
          onEnter={handleSend}
          disabled={!target || sending || isSystemChannel}
          rows={1}
          members={members}
        />
        <button
          className="message-input-send"
          onClick={handleSend}
          disabled={!target || sending || isSystemChannel || (!content.trim() && pendingFiles.length === 0)}
        >
          {sending ? '...' : 'Send'}
        </button>
      </div>
      {selectedChannel && !isSystemChannel && (
        <div className="message-input-footer">
          <label className="task-checkbox-label">
            <input
              type="checkbox"
              checked={alsoTask}
              onChange={(e) => setAlsoTask(e.target.checked)}
            />
            Also create as a task
          </label>
        </div>
      )}
    </div>
  )
}
