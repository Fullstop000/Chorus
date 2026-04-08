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

  // Collapsed mode: show only the selected card with a "Change" button.
  if (selected) {
    return (
      <div className="template-gallery-collapsed">
        <TemplateCard template={selected} isSelected onClick={() => {}} />
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
        <Input
          value={search}
          onChange={e => setSearch(e.target.value)}
          placeholder="Search templates..."
          className="template-search"
        />
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
        <Button variant="ghost" size="sm" onClick={handleSurprise} title="Surprise me">
          <Shuffle size={13} />
          <span className="template-surprise-label">Surprise me</span>
        </Button>
      </div>

      <div className="template-grid" role="listbox" aria-label="Agent templates">
        {filtered.length === 0 && (
          <div className="template-empty">No templates match your search.</div>
        )}
        {filtered.map(template => (
          <TemplateCard
            key={template.id}
            template={template}
            isSelected={false}
            onClick={() => onSelect(template)}
          />
        ))}
      </div>
    </div>
  )
}

function TemplateCard({
  template,
  isSelected,
  onClick,
}: {
  template: AgentTemplate
  isSelected: boolean
  onClick: () => void
}) {
  return (
    <button
      className={`template-card ${isSelected ? 'selected' : ''}`}
      onClick={onClick}
      role="option"
      aria-selected={isSelected}
      aria-label={`${template.name} - ${template.vibe ?? template.description ?? ''}`}
    >
      <span
        className="template-card-emoji"
        style={{ borderLeftColor: template.color ?? 'var(--color-border)' }}
      >
        {template.emoji ?? '🤖'}
      </span>
      <span className="template-card-body">
        <span className="template-card-name">{template.name}</span>
        <span className="template-card-vibe">{template.vibe ?? template.description ?? ''}</span>
        <span className="template-card-tag">{template.category}</span>
      </span>
    </button>
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
