import { Loader2, ChevronDown } from "lucide-react"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { cn } from "@/lib/cn"

function SelectSkeleton({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "flex w-full items-center justify-between",
        "min-h-[46px] px-3 py-2",
        "border border-input rounded-none bg-muted",
        "opacity-60",
        className
      )}
    >
      <span className="flex items-center gap-2 text-muted-foreground text-[13px] font-mono">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading...
      </span>
      <ChevronDown className="h-4 w-4 opacity-50" />
    </div>
  )
}

export interface AsyncSelectProps {
  value: string
  onValueChange: (value: string) => void
  options: { value: string; label: string }[]
  placeholder?: string
  isLoading?: boolean
  error?: string | null
  emptyMessage?: string
  disabled?: boolean
  className?: string
}

function AsyncSelect({
  value,
  onValueChange,
  options,
  placeholder,
  isLoading,
  error,
  emptyMessage = "No options available",
  disabled,
  className,
}: AsyncSelectProps) {
  if (isLoading && !value) {
    return <SelectSkeleton className={className} />
  }

  return (
    <Select value={value} onValueChange={onValueChange} disabled={disabled || isLoading}>
      <SelectTrigger className={cn(error && "border-destructive bg-destructive/10", className)}>
        <SelectValue placeholder={placeholder} />
        {isLoading && <Loader2 className="h-4 w-4 animate-spin ml-auto mr-2" />}
      </SelectTrigger>
      <SelectContent>
        {options.length === 0 ? (
          <div className="px-3 py-2 text-muted-foreground text-sm">{emptyMessage}</div>
        ) : (
          options.map((opt) => (
            <SelectItem key={opt.value} value={opt.value}>
              {opt.label}
            </SelectItem>
          ))
        )}
      </SelectContent>
    </Select>
  )
}

export interface RuntimeStatusInfo {
  runtime: string
  installed: boolean
  authenticated: boolean
  label?: string
}

export interface RuntimeSelectProps {
  value: string
  onValueChange: (value: string) => void
  runtimes: RuntimeStatusInfo[]
  isLoading?: boolean
  error?: string | null
  className?: string
}

function RuntimeSelect({
  value,
  onValueChange,
  runtimes,
  isLoading,
  error,
  className,
}: RuntimeSelectProps) {
  const options = runtimes.map((rt) => ({
    value: rt.runtime,
    label: rt.label || rt.runtime,
  }))

  return (
    <AsyncSelect
      value={value}
      onValueChange={onValueChange}
      options={options}
      placeholder="Select runtime"
      isLoading={isLoading}
      error={error}
      emptyMessage="No runtimes available"
      className={className}
    />
  )
}

export { AsyncSelect, RuntimeSelect, SelectSkeleton }
