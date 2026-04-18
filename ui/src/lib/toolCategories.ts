import {
  FileText,
  Search,
  Terminal,
  Globe,
  Cpu,
  Wrench,
  type LucideIcon,
} from 'lucide-react'

export type ToolCategory = 'file' | 'search' | 'terminal' | 'net' | 'compute' | 'other'

interface CategoryRule {
  category: ToolCategory
  icon: LucideIcon
  label: string
  patterns: string[]
}

const rules: CategoryRule[] = [
  {
    category: 'file',
    icon: FileText,
    label: 'File operations',
    patterns: ['read_file', 'write_file', 'edit', 'list_dir', 'ls'],
  },
  {
    category: 'search',
    icon: Search,
    label: 'Search',
    patterns: ['search', 'grep', 'find', 'glob'],
  },
  {
    category: 'terminal',
    icon: Terminal,
    label: 'Terminal commands',
    patterns: ['bash', 'shell', 'exec', 'run', 'command'],
  },
  {
    category: 'net',
    icon: Globe,
    label: 'Network requests',
    patterns: ['http', 'fetch', 'curl', 'api', 'web'],
  },
  {
    category: 'compute',
    icon: Cpu,
    label: 'Compute',
    patterns: ['python', 'eval', 'calculate'],
  },
  {
    category: 'other',
    icon: Wrench,
    label: 'Other tools',
    patterns: [],
  },
]

const otherRule = rules.find((r) => r.category === 'other')!

export function classifyTool(toolName: string): { category: ToolCategory; icon: LucideIcon } {
  const lower = toolName.toLowerCase()
  for (const rule of rules) {
    if (rule.patterns.some((p) => lower.includes(p))) {
      return { category: rule.category, icon: rule.icon }
    }
  }
  return { category: otherRule.category, icon: otherRule.icon }
}

export function iconForCategory(category: string): LucideIcon {
  return rules.find((r) => r.category === category)?.icon ?? otherRule.icon
}

export function labelForCategory(category: string): string {
  return rules.find((r) => r.category === category)?.label ?? category
}
