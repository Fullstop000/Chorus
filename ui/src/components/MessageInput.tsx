import { useState, useRef, type KeyboardEvent } from 'react'
import { useApp, useTarget } from '../store'
import { sendMessage, createTasks, uploadFile } from '../api'

interface Props {
  onMessageSent?: () => void
}

export function MessageInput({ onMessageSent }: Props) {
  const { currentUser, selectedChannel } = useApp()
  const target = useTarget()
  const [content, setContent] = useState('')
  const [alsoTask, setAlsoTask] = useState(false)
  const [sending, setSending] = useState(false)
  const [pendingFiles, setPendingFiles] = useState<File[]>([])
  const fileInputRef = useRef<HTMLInputElement>(null)

  const placeholder = target
    ? `Message ${target}`
    : 'Select a channel to message'

  async function handleSend() {
    if (!target || !currentUser || (!content.trim() && pendingFiles.length === 0)) return
    setSending(true)
    try {
      // Upload files first
      const attachmentIds: string[] = []
      for (const file of pendingFiles) {
        const res = await uploadFile(currentUser, file)
        attachmentIds.push(res.id)
      }

      await sendMessage(currentUser, target, content.trim(), attachmentIds)

      if (alsoTask && selectedChannel && content.trim()) {
        await createTasks(currentUser, selectedChannel, [content.trim()])
      }

      setContent('')
      setPendingFiles([])
      setAlsoTask(false)
      onMessageSent?.()
    } catch (e) {
      console.error('Send failed:', e)
    } finally {
      setSending(false)
    }
  }

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      handleSend()
    }
  }

  function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? [])
    setPendingFiles((prev) => [...prev, ...files])
    if (fileInputRef.current) fileInputRef.current.value = ''
  }

  return (
    <div className="message-input-area">
      {pendingFiles.length > 0 && (
        <div className="message-input-files">
          {pendingFiles.map((f, i) => (
            <span key={i} className="file-chip">
              📎 {f.name}
              <button
                onClick={() => setPendingFiles((prev) => prev.filter((_, j) => j !== i))}
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
          disabled={!target}
          title="Attach file"
        >
          ⊕
        </button>
        <input
          ref={fileInputRef}
          type="file"
          multiple
          style={{ display: 'none' }}
          onChange={handleFileChange}
        />
        <textarea
          className="message-input-textarea"
          placeholder={placeholder}
          value={content}
          onChange={(e) => setContent(e.target.value)}
          onKeyDown={handleKeyDown}
          disabled={!target || sending}
          rows={1}
        />
        <button
          className="message-input-send"
          onClick={handleSend}
          disabled={!target || sending || (!content.trim() && pendingFiles.length === 0)}
        >
          {sending ? '...' : 'Send'}
        </button>
      </div>
      {selectedChannel && (
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
