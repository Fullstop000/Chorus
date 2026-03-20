import { useState, useEffect, useCallback } from 'react'
import { getAgentWorkspace } from '../api'
import './WorkspacePanel.css'

interface Props {
  agentName: string
}

export function WorkspacePanel({ agentName }: Props) {
  const [files, setFiles] = useState<string[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const res = await getAgentWorkspace(agentName)
      setFiles(res.files)
      setError(null)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [agentName])

  useEffect(() => {
    setLoading(true)
    load()
    const interval = setInterval(load, 10000)
    return () => clearInterval(interval)
  }, [load])

  if (loading && files.length === 0) {
    return (
      <div className="workspace-panel">
        <div className="workspace-header">
          <span className="workspace-title">Workspace</span>
        </div>
        <div className="workspace-empty">Loading...</div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="workspace-panel">
        <div className="workspace-header">
          <span className="workspace-title">Workspace</span>
        </div>
        <div className="workspace-empty" style={{ color: 'var(--accent)' }}>{error}</div>
      </div>
    )
  }

  return (
    <div className="workspace-panel">
      <div className="workspace-header">
        <span className="workspace-title">Workspace — {agentName}</span>
        <span className="workspace-path">~/.chorus/{agentName}/</span>
      </div>
      {files.length === 0 ? (
        <div className="workspace-empty">Workspace is empty.</div>
      ) : (
        <div className="workspace-tree">
          {files.map((file) => {
            const isDir = file.endsWith('/')
            const depth = file.split('/').length - (isDir ? 2 : 1)
            const name = file.split('/').filter(Boolean).pop() ?? file
            return (
              <div
                key={file}
                className={`workspace-entry ${isDir ? 'workspace-dir' : 'workspace-file'}`}
                style={{ paddingLeft: 12 + depth * 16 }}
              >
                <span className="workspace-icon">{isDir ? '📁' : fileIcon(name)}</span>
                <span className="workspace-name">{name}{isDir ? '/' : ''}</span>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}

function fileIcon(name: string): string {
  const ext = name.split('.').pop()?.toLowerCase() ?? ''
  switch (ext) {
    case 'rs': return '🦀'
    case 'ts': case 'tsx': return '📘'
    case 'js': case 'jsx': return '📜'
    case 'py': return '🐍'
    case 'md': return '📝'
    case 'json': return '🔧'
    case 'toml': case 'yaml': case 'yml': return '⚙️'
    case 'sh': return '💻'
    case 'lock': return '🔒'
    default: return '📄'
  }
}
