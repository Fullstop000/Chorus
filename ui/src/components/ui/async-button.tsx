import * as React from "react"
import { Loader2 } from "lucide-react"
import { Button, type ButtonProps } from "@/components/ui/button"

export interface AsyncButtonProps extends ButtonProps {
  isLoading?: boolean
  loadingText?: string
}

const AsyncButton = React.forwardRef<HTMLButtonElement, AsyncButtonProps>(
  ({ isLoading, loadingText, disabled, children, className, ...props }, ref) => (
    <Button
      ref={ref}
      disabled={isLoading || disabled}
      className={className}
      {...props}
    >
      {isLoading && <Loader2 className="h-4 w-4 animate-spin" />}
      {isLoading && loadingText ? loadingText : children}
    </Button>
  )
)
AsyncButton.displayName = "AsyncButton"

const IconButton = React.forwardRef<HTMLButtonElement, AsyncButtonProps>(
  ({ className, ...props }, ref) => (
    <AsyncButton ref={ref} size="icon" variant="ghost" className={className} {...props} />
  )
)
IconButton.displayName = "IconButton"

export { AsyncButton, IconButton }
