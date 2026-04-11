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
  patterns: string[]
}

const rules: CategoryRule[] = [
  { category: 'file', icon: FileText, patterns: ['read_file', 'write_file', 'edit', 'list_dir', 'ls'] },
  { category: 'search', icon: Search, patterns: ['search', 'grep', 'find', 'glob'] },
  { category: 'terminal', icon: Terminal, patterns: ['bash', 'shell', 'exec', 'run', 'command'] },
  { category: 'net', icon: Globe, patterns: ['http', 'fetch', 'curl', 'api', 'web'] },
  { category: 'compute', icon: Cpu, patterns: ['python', 'eval', 'calculate'] },
]

const defaultResult = { category: 'other' as ToolCategory, icon: Wrench }

export function classifyTool(toolName: string): { category: ToolCategory; icon: LucideIcon } {
  const lower = toolName.toLowerCase()
  for (const rule of rules) {
    if (rule.patterns.some(p => lower.includes(p))) {
      return { category: rule.category, icon: rule.icon }
    }
  }
  return defaultResult
}

export function iconForCategory(category: string): LucideIcon {
  return rules.find(r => r.category === category)?.icon ?? Wrench
}
