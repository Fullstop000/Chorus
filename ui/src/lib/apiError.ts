export type ApiErrorCode =
  | 'INTERNAL_ERROR'
  | 'AGENT_NAME_TAKEN'
  | 'CHANNEL_NAME_TAKEN'
  | 'TEAM_NAME_TAKEN'
  | 'AGENT_RESTART_FAILED'
  | 'AGENT_DELETE_WORKSPACE_CLEANUP_FAILED'
  | 'CHANNEL_OPERATION_UNSUPPORTED'
  | 'MESSAGE_NOT_A_MEMBER'

export class ApiError extends Error {
  readonly status: number
  readonly code?: ApiErrorCode

  constructor(status: number, message: string, code?: ApiErrorCode) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
    }
}

export function isApiError(e: unknown): e is ApiError {
    return e instanceof ApiError
}

export function isNameTaken(e: unknown): e is ApiError {
  return (
    isApiError(e) &&
    (e.code === 'AGENT_NAME_TAKEN' ||
        e.code === 'CHANNEL_NAME_TAKEN' ||
        e.code === 'TEAM_NAME_TAKEN')
  )
}

export function isAgentRestartFailure(e: unknown): e is ApiError {
    return isApiError(e) && e.code === 'AGENT_RESTART_FAILED'
}

export function isUnsupportedChannelOp(e: unknown): e is ApiError {
    return isApiError(e) && e.code === 'CHANNEL_OPERATION_UNSUPPORTED'
}

export function isMembershipError(e: unknown): e is ApiError {
    return isApiError(e) && e.code === 'MESSAGE_NOT_A_MEMBER'
}
