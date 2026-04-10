import { useState, useMemo } from 'react'
import { Shuffle } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { AgentTemplate, TemplateCategory } from '../../hooks/useTemplates'
import './TemplateGallery.css'

interface Props {
  categories: TemplateCategory[]
  allTemplates: AgentTemplate[]
  selected: AgentTemplate | null
  onSelect: (template: AgentTemplate | null) => void
}

export function TemplateGallery({ categories, allTemplates, selected, onSelect }: Props) {
  const [search, setSearch] = useState('')
  const [activeCategory, setActiveCategory] = useState<string | null>(null)

  const categoryNames = useMemo(() => categories.map(c => c.name), [categories])

  const filtered = useMemo(() => {
    let templates = activeCategory
      ? allTemplates.filter(t => t.category === activeCategory)
      : allTemplates
    if (search.trim()) {
      const q = search.toLowerCase()
      templates = templates.filter(
        t => t.name.toLowerCase().includes(q) || (t.vibe ?? '').toLowerCase().includes(q)
      )
    }
    return templates
  }, [allTemplates, activeCategory, search])

  function handleSurprise() {
    if (allTemplates.length === 0) return
    const random = allTemplates[Math.floor(Math.random() * allTemplates.length)]
    onSelect(random)
  }

  // Collapsed mode: show only the selected row with a "Change" button.
  if (selected) {
    return (
      <div className="template-gallery-collapsed">
        <TemplateRow template={selected} />
        <Button
          variant="ghost"
          size="sm"
          className="template-change-btn"
          onClick={() => onSelect(null)}
        >
          Change template
        </Button>
      </div>
    )
  }

  return (
    <div className="template-gallery">
      <div className="template-filter-bar">
        <div className="template-filter-row">
          <Input
            value={search}
            onChange={e => setSearch(e.target.value)}
            placeholder="Search templates..."
            className="template-search"
          />
          <Button variant="ghost" size="sm" onClick={handleSurprise} title="Surprise me">
            <Shuffle size={13} />
            <span className="template-surprise-label">Surprise me</span>
          </Button>
        </div>
        <div className="template-category-tags">
          <button
            className={`template-tag ${activeCategory === null ? 'active' : ''}`}
            onClick={() => setActiveCategory(null)}
          >
            All
          </button>
          {categoryNames.map(name => (
            <button
              key={name}
              className={`template-tag ${activeCategory === name ? 'active' : ''}`}
              onClick={() => setActiveCategory(name)}
            >
              {name}
            </button>
          ))}
        </div>
      </div>

      <div className="template-list" role="listbox" aria-label="Agent templates">
        {filtered.length === 0 && (
          <div className="template-empty">No templates match your search.</div>
        )}
        {filtered.map(template => (
          <TemplateRow
            key={template.id}
            template={template}
            onClick={() => onSelect(template)}
          />
        ))}
      </div>
    </div>
  )
}

function TemplateRow({
  template,
  onClick,
}: {
  template: AgentTemplate
  onClick?: () => void
}) {
  return (
    <div
      className="template-row"
      onClick={onClick}
      role="option"
      aria-label={`${template.name} - ${template.vibe ?? template.description ?? ''}`}
      tabIndex={0}
      onKeyDown={e => { if (onClick && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); onClick() } }}
    >
      <span className="template-row-emoji">{template.emoji ?? '🤖'}</span>
      <span className="template-row-name">{template.name}</span>
      <span className="template-row-desc">{template.vibe ?? template.description ?? ''}</span>
      <span className="template-row-badge">{template.category}</span>
    </div>
  )
}

export function TemplatePreview({ template }: { template: AgentTemplate }) {
  const preview = template.prompt_body.length > 200
    ? template.prompt_body.slice(0, 200) + '...'
    : template.prompt_body

  return (
    <div className="template-preview">
      <span className="template-preview-label">Prompt preview</span>
      <p className="template-preview-text">{preview}</p>
    </div>
  )
}
