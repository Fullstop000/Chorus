/**
 * Async Button Component
 * 
 * Chorus-specific wrapper around shadcn/ui Button that adds:
 * - Loading state with spinner
 * - Loading text support
 * - Brutalist styling variants
 */

import * as React from "react"
import { Loader2 } from "lucide-react"
import { cn } from "@/lib/utils"
import { Button, ButtonProps } from "./button"

export interface AsyncButtonProps extends ButtonProps {
  isLoading?: boolean
  loadingText?: string
}

/**
 * AsyncButton - Button with loading state
 * 
 * Usage:
 * ```tsx
 * <AsyncButton
 *   onClick={handleSubmit}
 *   isLoading={isSubmitting}
 *   loadingText="Saving..."
 * >
 *   Save Agent
 * </AsyncButton>
 * ```
 */
export const AsyncButton = React.forwardRef<
  HTMLButtonElement,
  AsyncButtonProps
>(({ className, variant, size, isLoading, loadingText, children, ...props }, ref) => {
  return (
    <Button
      ref={ref}
      variant={variant}
      size={size}
      className={cn(
        // Brutalist button styles
        "font-mono text-[12px] uppercase tracking-[0.05em]",
        "relative overflow-hidden",
        variant === 'default' && [
          "border border-[var(--line-strong)] bg-[var(--bg-panel-strong)]",
          "hover:bg-[var(--accent)] hover:text-[#f8f6f1] hover:border-[var(--accent)]",
          "active:translate-y-[1px]",
        ],
        className
      )}
      disabled={isLoading}
      {...props}
    >
      {isLoading && <Loader2 className="h-4 w-4 animate-spin" />}
      {isLoading && loadingText ? loadingText : children}
    </Button>
  )
})
AsyncButton.displayName = "AsyncButton"

/**
 * IconButton - Square button for icons
 */
export interface IconButtonProps extends Omit<AsyncButtonProps, 'size' | 'children'> {
  icon: React.ReactNode
  label: string
}

export const IconButton = React.forwardRef<HTMLButtonElement, IconButtonProps>(
  ({ icon, label, className, variant = "ghost", ...props }, ref) => (
    <AsyncButton
      ref={ref}
      variant={variant}
      size="icon"
      className={cn("h-9 w-9 shrink-0", className)}
      aria-label={label}
      {...props}
    >
      {icon}
    </AsyncButton>
  )
)
IconButton.displayName = "IconButton"
