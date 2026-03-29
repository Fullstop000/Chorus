import { useEffect, useState, useRef } from 'react'
import { Paperclip, Plus } from 'lucide-react'
import { useApp } from '../store'
import { useHistory } from '../hooks/useHistory'
import { sendMessage, createTasks, uploadFile } from '../api'
import { MentionTextarea } from './MentionTextarea'
import type { MentionMember } from './MentionTextarea'
import { ToastRegion } from './ToastRegion'

interface Props {
  target: string | null
  history: ReturnType<typeof useHistory>
}

export function MessageInput({ target, history }: Props) {
  const { currentUser, selectedChannel, serverInfo, agents, teams } = useApp()
  const [content, setContent] = useState('')
  const [alsoTask, setAlsoTask] = useState(false)
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [pendingFiles, setPendingFiles] = useState<File[]>([])
  const [toasts, setToasts] = useState<Array<{ id: string; message: string }>>([])
  const fileInputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (toasts.length === 0) return
    const timer = window.setTimeout(() => {
      setToasts((current) => current.slice(1))
    }, 4000)
    return () => window.clearTimeout(timer)
  }, [toasts])

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
    let optimisticHandle: ReturnType<typeof history.addOptimisticMessage> | null = null
    const trimmedContent = content.trim()
    try {
      // Upload files first
      const attachmentIds: string[] = []
      for (const file of pendingFiles) {
        const res = await uploadFile(currentUser, file)
        attachmentIds.push(res.id)
      }

      const handle = history.addOptimisticMessage({
        content: trimmedContent,
        attachments: attachmentIds.map((id, index) => ({
          id,
          filename: pendingFiles[index]?.name ?? 'attachment',
        })),
      })
      optimisticHandle = handle

      const sendAck = await sendMessage(currentUser, target, trimmedContent, attachmentIds, {
        clientNonce: handle.clientNonce,
        suppressAgentDelivery: alsoTask && !!selectedChannel,
      })
      history.ackOptimisticMessage(handle, {
        messageId: sendAck.messageId,
        seq: sendAck.seq,
        createdAt: sendAck.createdAt,
        clientNonce: sendAck.clientNonce,
      })
      setContent('')
      setPendingFiles([])
      setAlsoTask(false)
    } catch (e) {
      console.error('Send failed:', e)
      const message = e instanceof Error ? e.message : String(e)
      if (optimisticHandle) {
        history.failOptimisticMessage(optimisticHandle, message)
      }
      setError(message)
      setToasts((current) => [
        ...current,
        { id: `send-failed-${Date.now()}`, message: 'Message failed to send' },
      ])
    } finally {
      setSending(false)
    }

    if (alsoTask && selectedChannel && trimmedContent) {
      try {
        await createTasks(currentUser, selectedChannel, [trimmedContent])
      } catch (taskError) {
        const message = taskError instanceof Error ? taskError.message : String(taskError)
        setError(message)
        setToasts((current) => [
          ...current,
          { id: `task-create-failed-${Date.now()}`, message: 'Task creation failed' },
        ])
      }
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
      <ToastRegion
        toasts={toasts}
        onDismiss={(id) => setToasts((current) => current.filter((toast) => toast.id !== id))}
      />
    </div>
  )
}
