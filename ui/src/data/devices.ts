import { get, post, del } from './client'

export interface Device {
  machine_id: string
  hostname_hint: string | null
  first_seen_at: string
  last_seen_at: string
  disconnected_at: string | null
  kicked_at: string | null
  active: boolean
}

export interface MintResponse {
  script: string
  host: string
}

export function listDevices(): Promise<Device[]> {
  return get<Device[]>('/api/devices')
}

export function mintDevice(): Promise<MintResponse> {
  return post<MintResponse>('/api/devices/mint')
}

export function rotateDevice(): Promise<MintResponse> {
  return post<MintResponse>('/api/devices/rotate')
}

export function kickDevice(machineId: string): Promise<void> {
  return del<void>(`/api/devices/${encodeURIComponent(machineId)}`)
}

export function forgetDevice(machineId: string): Promise<void> {
  return del<void>(`/api/devices/${encodeURIComponent(machineId)}?forget=1`)
}

export interface HealthInfo {
  status: string
  dev_auth: boolean
}

export function getHealth(): Promise<HealthInfo> {
  return get<HealthInfo>('/health')
}
