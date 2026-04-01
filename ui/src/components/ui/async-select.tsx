/**
 * Async Select Component
 * 
 * Chorus-specific wrapper around shadcn/ui Select that adds:
 * - Loading state with skeleton UI
 * - Error state handling
 * - Empty state
 * - Integration with runtime status data
 */

import * as React from "react"
import { Loader2 } from "lucide-react"
import { cn } from "@/lib/utils"
import type { RuntimeStatusInfo } from "@/types"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./select"

export interface AsyncSelectOption {
  value: string
  label: React.ReactNode
  disabled?: boolean
}

export interface AsyncSelectProps {
  value: string
  onValueChange: (value: string) => void
  options: AsyncSelectOption[]
  placeholder?: string
  isLoading?: boolean
  error?: string | null
  emptyText?: string
  disabled?: boolean
  className?: string
  triggerClassName?: string
}

/**
 * SelectSkeleton - Loading placeholder for Select
 */
function SelectSkeleton({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "flex h-10 w-full items-center justify-between gap-2",
        "border border-[var(--line-strong)] bg-[var(--bg-panel-muted)]",
        "px-3 py-2",
        className
      )}
    >
      <span className="flex items-center gap-2 text-sm text-[var(--text-muted)]">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading...
      </span>
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width="24"
        height="24"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="h-4 w-4 opacity-50"
      >
        <path d="m6 9 6 6 6-6" />
      </svg>
    </div>
  )
}

/**
 * AsyncSelect - Select with loading and error states
 * 
 * Usage:
 * ```tsx
 * <AsyncSelect
 *   value={selectedRuntime}
 *   onValueChange={setSelectedRuntime}
 *   options={[
 *     { value: 'claude', label: 'Claude Code' },
 *       { value: 'codex', label: 'Codex CLI' },
 *     ]}
 *   isLoading={isLoadingRuntimes}
 *   error={runtimesError}
 * />
 * ```
 */
export function AsyncSelect({
  value,
  onValueChange,
  options,
  placeholder = "Select an option...",
  isLoading,
  error,
  emptyText = "No options available",
  disabled,
  className,
  triggerClassName,
}: AsyncSelectProps) {
  // Show skeleton while loading and no value selected
  if (isLoading && !value) {
    return <SelectSkeleton className={className} />
  }

  return (
    <Select
      value={value}
      onValueChange={onValueChange}
      disabled={disabled || isLoading}
    >
      <SelectTrigger
        className={cn(
          "h-10 w-full",
          error && "border-[#c67a18] bg-[rgba(198,122,24,0.08)]",
          triggerClassName
        )}
      >
        {isLoading ? (
          <span className="flex items-center gap-2 text-[var(--text-muted)]">
            <Loader2 className="h-4 w-4 animate-spin" />
            Loading...
          </span>
        ) : (
          <SelectValue placeholder={placeholder} />
        )}
      </SelectTrigger>
      <SelectContent className={className}>
        {options.length === 0 ? (
          <div className="px-3 py-2 text-sm text-[var(--text-muted)]">
            {emptyText}
          </div>
        ) : (
          options.map((option) => (
            <SelectItem
              key={option.value}
              value={option.value}
              disabled={option.disabled}
            >
              {option.label}
            </SelectItem>
          ))
        )}
      </SelectContent>
    </Select>
  )
}

/**
 * RuntimeSelect - Specialized select for runtime selection with status
 * 
 * Usage:
 * ```tsx
 * <RuntimeSelect
 *   value={runtime}
 *   onValueChange={setRuntime}
 *   runtimes={[
 *     { runtime: 'claude', installed: true, authStatus: 'authed' },
 *     { runtime: 'codex', installed: false },
 *   ]}
 *   isLoading={isLoading}
 * />
 * ```
 */
export interface RuntimeSelectProps
  extends Omit<AsyncSelectProps, 'options'> {
  runtimes: RuntimeStatusInfo[]
}

function getRuntimeLabel(runtime: string): string {
  switch (runtime) {
    case 'claude':
      return 'Claude Code'
    case 'codex':
      return 'Codex CLI'
    case 'kimi':
      return 'Kimi CLI'
    case 'opencode':
      return 'OpenCode'
    default:
      return runtime
  }
}

function getRuntimeStatusLabel(info: RuntimeStatusInfo | undefined): string {
  if (!info) return 'status unavailable'
  if (!info.installed) return 'not installed'
  if (info.authStatus === 'authed') return 'signed in'
  return 'not signed in'
}

export function RuntimeSelect({
  runtimes,
  ...props
}: RuntimeSelectProps) {
  const options: AsyncSelectOption[] = React.useMemo(
    () =>
      runtimes.map((rt) => ({
        value: rt.runtime,
        label: (
          <span className="flex items-center gap-2">
            <span>{getRuntimeLabel(rt.runtime)}</span>
            <span className="text-[var(--text-muted)]">
              · {getRuntimeStatusLabel(rt)}
            </span>
          </span>
        ),
      })),
    [runtimes]
  )

  return <AsyncSelect {...props} options={options} />
}
