import { get, post, patch, del } from './client'
import { queryOptions } from '@tanstack/react-query'
import type {
  CreateTeamRequest,
  UpdateTeamRequest,
  AddTeamMemberRequest,
} from './requests'

// ── Types (source of truth) ──

export interface Team {
  id: string
  name: string
  display_name: string
  channel_id?: string | null
  created_at: string
}

export interface TeamMember {
  team_id: string
  member_name: string
  member_type: 'agent' | 'human'
  member_id: string
  role: string
  joined_at: string
}

export interface TeamResponse {
  team: Team
  members: TeamMember[]
}

// ── API functions ──

export function listTeams(): Promise<Team[]> {
  return get('/api/teams')
}

export function getTeam(teamId: string): Promise<TeamResponse> {
  return get(`/api/teams/${encodeURIComponent(teamId)}`)
}

export function createTeam(payload: CreateTeamRequest): Promise<TeamResponse> {
  return post('/api/teams', payload)
}

export function updateTeam(
  teamId: string,
  payload: {
    display_name?: string
  }
): Promise<Team> {
  return patch(`/api/teams/${encodeURIComponent(teamId)}`, payload as UpdateTeamRequest)
}

export function deleteTeam(teamId: string): Promise<void> {
  return del(`/api/teams/${encodeURIComponent(teamId)}`)
}

export function addTeamMember(
  teamId: string,
  member: {
    member_name: string
    member_type: 'agent' | 'human'
    member_id: string
    role: string
  }
): Promise<void> {
  return post(`/api/teams/${encodeURIComponent(teamId)}/members`, member as AddTeamMemberRequest)
}

export function removeTeamMember(teamId: string, memberName: string): Promise<void> {
  return del(`/api/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(memberName)}`)
}

// ── Query definitions ──

export const teamQueryKeys = {
  teams: ['teams'] as const,
} as const

export const teamsQuery = (currentUser: string) =>
  queryOptions({
    queryKey: teamQueryKeys.teams,
    queryFn: listTeams,
    enabled: !!currentUser,
  })
