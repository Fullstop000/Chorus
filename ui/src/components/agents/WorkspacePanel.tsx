import { useState, useEffect, useCallback } from 'react'
import ReactMarkdown from 'react-markdown'
import {
  ChevronDown,
  ChevronRight,
  Copy,
  File,
  FileCode2,
  FileJson,
  Folder,
  FolderOpen,
  RefreshCw,
  ScrollText,
  Settings2,
} from 'lucide-react'
import { getAgentWorkspace, getAgentWorkspaceFile } from '../../data'
import './WorkspacePanel.css'

interface Props {
  agentName: string
}

interface WorkspaceNode {
  name: string
  path: string
  isDir: boolean
  children: WorkspaceNode[]
}

type PreviewMode = 'raw' | 'preview'

export function WorkspacePanel({ agentName }: Props) {
  const [files, setFiles] = useState<string[]>([])
  const [workspacePath, setWorkspacePath] = useState('')
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(new Set())
  const [previewContent, setPreviewContent] = useState('')
  const [previewTruncated, setPreviewTruncated] = useState(false)
  const [previewSizeBytes, setPreviewSizeBytes] = useState<number | null>(null)
  const [previewModifiedMs, setPreviewModifiedMs] = useState<number | null>(null)
  const [previewMode, setPreviewMode] = useState<PreviewMode>('preview')
  const [previewLoading, setPreviewLoading] = useState(false)
  const [previewError, setPreviewError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const res = await getAgentWorkspace(agentName)
      setFiles(res.files)
      setWorkspacePath(res.path)
      setExpandedPaths((current) => {
        const next = new Set(current)
        for (const file of res.files) {
          if (file.endsWith('/')) {
            const depth = file.split('/').filter(Boolean).length
            if (depth === 1) next.add(file)
          }
        }
        return next
      })
      setError(null)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [agentName])

  const loadPreview = useCallback(async (path: string) => {
    setPreviewLoading(true)
    try {
      const res = await getAgentWorkspaceFile(agentName, path)
      setPreviewContent(res.content)
      setPreviewTruncated(res.truncated)
      setPreviewSizeBytes(res.sizeBytes)
      setPreviewModifiedMs(res.modifiedMs ?? null)
      setPreviewError(null)
    } catch (e) {
      setPreviewContent('')
      setPreviewTruncated(false)
      setPreviewSizeBytes(null)
      setPreviewModifiedMs(null)
      setPreviewError(String(e))
    } finally {
      setPreviewLoading(false)
    }
  }, [agentName])

  useEffect(() => {
    setLoading(true)
    setSelectedPath(null)
    setPreviewContent('')
    setPreviewTruncated(false)
    setPreviewSizeBytes(null)
    setPreviewModifiedMs(null)
    setPreviewError(null)
    load()
    const interval = setInterval(load, 10000)
    return () => clearInterval(interval)
  }, [load])

  useEffect(() => {
    if (selectedPath == null) {
      setPreviewContent('')
      setPreviewTruncated(false)
      setPreviewSizeBytes(null)
      setPreviewModifiedMs(null)
      setPreviewError(null)
      setPreviewLoading(false)
      return
    }
    if (!files.includes(selectedPath)) {
      setSelectedPath(null)
      return
    }
    loadPreview(selectedPath)
  }, [files, loadPreview, selectedPath])

  const copyWorkspacePath = useCallback(async () => {
    if (!workspacePath || typeof navigator === 'undefined' || !navigator.clipboard?.writeText) return
    try {
      await navigator.clipboard.writeText(workspacePath)
    } catch {
      // Ignore clipboard failures in unsupported contexts.
    }
  }, [workspacePath])

  if (loading && files.length === 0) {
    return (
      <div className="workspace-panel">
        <div className="workspace-toolbar workspace-toolbar-loading">
          <span className="workspace-location">Loading workspace…</span>
        </div>
        <div className="workspace-empty">Loading…</div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="workspace-panel">
        <div className="workspace-toolbar workspace-toolbar-loading">
          <span className="workspace-location">Workspace unavailable</span>
        </div>
        <div className="workspace-empty workspace-error">{error}</div>
      </div>
    )
  }

  const tree = buildTree(files)
  const isMarkdown = selectedPath?.toLowerCase().endsWith('.md') ?? false
  const metadata = [
    previewSizeBytes != null ? formatFileSize(previewSizeBytes) : null,
    previewModifiedMs != null ? formatTimestamp(previewModifiedMs) : null,
  ].filter((value): value is string => value != null)

  return (
    <div className="workspace-panel">
      <div className="workspace-toolbar">
        <div className="workspace-toolbar-path">
          <span className="workspace-toolbar-kicker">[workspace::path]</span>
          <span className="workspace-location">{workspacePath || '(workspace unavailable)'}</span>
        </div>
        <button className="workspace-toolbar-btn" type="button" onClick={copyWorkspacePath} aria-label="Copy workspace path">
          <Copy size={18} />
        </button>
      </div>

      <div className="workspace-shell">
        <aside className="workspace-sidebar">
          <div className="workspace-sidebar-header">
            <div className="workspace-sidebar-copy">
              <span className="workspace-sidebar-kicker">[tree::index]</span>
              <span className="workspace-sidebar-title">Workspace</span>
            </div>
            <button className="workspace-icon-btn" type="button" onClick={load} aria-label="Refresh workspace">
              <RefreshCw size={18} />
            </button>
          </div>

          <div className="workspace-tree">
            {tree.length === 0 ? (
              <div className="workspace-tree-empty">Workspace is empty.</div>
            ) : (
              tree.map((node) => (
                <TreeRow
                  key={node.path}
                  node={node}
                  level={0}
                  expandedPaths={expandedPaths}
                  selectedPath={selectedPath}
                  onToggle={(path) => {
                    setExpandedPaths((current) => {
                      const next = new Set(current)
                      if (next.has(path)) next.delete(path)
                      else next.add(path)
                      return next
                    })
                  }}
                  onSelect={setSelectedPath}
                />
              ))
            )}
          </div>
        </aside>

        <section className="workspace-preview">
          <div className="workspace-preview-header">
            <div className="workspace-preview-meta">
              <span className="workspace-preview-kicker">[preview::file]</span>
              <div className="workspace-preview-heading-row">
                <span className="workspace-preview-title">{selectedPath ?? 'Preview'}</span>
                {metadata.map((item) => (
                  <span key={item} className="workspace-preview-detail">{item}</span>
                ))}
              </div>
            </div>
            <div className="workspace-preview-actions">
              <button
                className={`workspace-mode-btn ${previewMode === 'raw' ? 'workspace-mode-btn-active' : ''}`}
                type="button"
                onClick={() => setPreviewMode('raw')}
              >
                Raw
              </button>
              <button
                className={`workspace-mode-btn ${previewMode === 'preview' ? 'workspace-mode-btn-active' : ''}`}
                type="button"
                onClick={() => setPreviewMode('preview')}
              >
                Preview
              </button>
            </div>
          </div>

          {selectedPath == null ? (
            <div className="workspace-preview-empty" />
          ) : previewLoading ? (
            <div className="workspace-preview-state">Loading file…</div>
          ) : previewError ? (
            <div className="workspace-preview-state workspace-error">{previewError}</div>
          ) : (
            <div className="workspace-preview-body">
              {previewTruncated ? (
                <div className="workspace-preview-banner">Preview limited to the first 100 KB.</div>
              ) : null}
              {previewMode === 'preview' && isMarkdown ? (
                <div className="workspace-markdown">
                  <ReactMarkdown>{previewContent}</ReactMarkdown>
                </div>
              ) : (
                <pre className="workspace-preview-content">{previewContent}</pre>
              )}
            </div>
          )}
        </section>
      </div>
    </div>
  )
}

function TreeRow({
  node,
  level,
  expandedPaths,
  selectedPath,
  onToggle,
  onSelect,
}: {
  node: WorkspaceNode
  level: number
  expandedPaths: Set<string>
  selectedPath: string | null
  onToggle: (path: string) => void
  onSelect: (path: string) => void
}) {
  const expanded = node.isDir ? expandedPaths.has(node.path) : false
  const selected = !node.isDir && selectedPath === node.path

  return (
    <>
      <button
        type="button"
        className={`workspace-row ${selected ? 'workspace-row-selected' : ''}`}
        style={{ paddingLeft: `${20 + level * 18}px` }}
        onClick={() => {
          if (node.isDir) onToggle(node.path)
          else onSelect(node.path)
        }}
      >
        <span className="workspace-row-caret">
          {node.isDir ? (expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />) : null}
        </span>
        <span className="workspace-row-icon">{node.isDir ? (expanded ? <FolderOpen size={18} /> : <Folder size={18} />) : fileIcon(node.name)}</span>
        <span className="workspace-row-label">{node.name}</span>
      </button>

      {node.isDir && expanded
        ? node.children.map((child) => (
            <TreeRow
              key={child.path}
              node={child}
              level={level + 1}
              expandedPaths={expandedPaths}
              selectedPath={selectedPath}
              onToggle={onToggle}
              onSelect={onSelect}
            />
          ))
        : null}
    </>
  )
}

function buildTree(files: string[]): WorkspaceNode[] {
  const roots: WorkspaceNode[] = []

  for (const entry of files) {
    const isDir = entry.endsWith('/')
    const parts = entry.split('/').filter(Boolean)
    let siblings = roots

    for (let index = 0; index < parts.length; index += 1) {
      const name = parts[index]
      const atLeaf = index === parts.length - 1
      const nodeIsDir = !atLeaf || isDir
      const nodePath = parts.slice(0, index + 1).join('/') + (nodeIsDir ? '/' : '')

      let node = siblings.find((item) => item.path === nodePath)
      if (node == null) {
        node = { name, path: nodePath, isDir: nodeIsDir, children: [] }
        siblings.push(node)
      }
      siblings = node.children
    }
  }

  sortNodes(roots)
  return roots
}

function sortNodes(nodes: WorkspaceNode[]) {
  nodes.sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
    return a.name.localeCompare(b.name)
  })
  for (const node of nodes) sortNodes(node.children)
}

function fileIcon(name: string) {
  const ext = name.split('.').pop()?.toLowerCase() ?? ''
  switch (ext) {
    case 'md':
      return <ScrollText size={18} />
    case 'json':
      return <FileJson size={18} />
    case 'toml':
    case 'yaml':
    case 'yml':
      return <Settings2 size={18} />
    case 'rs':
    case 'ts':
    case 'tsx':
    case 'js':
    case 'jsx':
    case 'py':
    case 'sh':
      return <FileCode2 size={18} />
    default:
      return <File size={18} />
  }
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function formatTimestamp(timestampMs: number): string {
  const date = new Date(timestampMs)
  const year = date.getFullYear()
  const month = date.getMonth() + 1
  const day = date.getDate()
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')
  return `${year}/${month}/${day} ${hours}:${minutes}:${seconds}`
}
