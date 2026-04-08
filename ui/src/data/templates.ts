import { get, post } from './client'

// ── Types ──

export interface AgentTemplate {
  id: string
  name: string
  emoji?: string
  color?: string
  vibe?: string
  description?: string
  category: string
  suggested_runtime: string
  prompt_body: string
}

export interface TemplateCategory {
  name: string
  templates: AgentTemplate[]
}

export interface TemplatesResponse {
  categories: TemplateCategory[]
}

export interface LaunchTrioAgent {
  id: string
  name: string
  display_name: string
}

export interface LaunchTrioResponse {
  channel_id: string
  agents: LaunchTrioAgent[]
  errors?: { template_id: string; error: string }[]
}

// ── API calls ──

export async function listTemplates(): Promise<TemplatesResponse> {
  return get<TemplatesResponse>('/api/templates')
}

export async function launchTrio(templateIds: string[]): Promise<LaunchTrioResponse> {
  return post<LaunchTrioResponse>('/api/templates/launch-trio', {
    template_ids: templateIds,
  })
}
