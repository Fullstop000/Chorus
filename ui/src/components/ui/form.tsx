import * as React from "react"
import { cn } from "@/lib/cn"
import { Label } from "@/components/ui/label"

const FormField = React.forwardRef<
  HTMLDivElement,
  React.HTMLAttributes<HTMLDivElement>
>(({ className, ...props }, ref) => (
  <div ref={ref} className={cn("mb-[10px]", className)} {...props} />
))
FormField.displayName = "FormField"

interface FormLabelProps extends React.ComponentPropsWithoutRef<typeof Label> {
  required?: boolean
}

const FormLabel = React.forwardRef<
  React.ElementRef<typeof Label>,
  FormLabelProps
>(({ className, required, children, ...props }, ref) => (
  <Label ref={ref} className={className} {...props}>
    {children}
    {required && <span className="text-destructive ml-1">*</span>}
  </Label>
))
FormLabel.displayName = "FormLabel"

const FormDescription = React.forwardRef<
  HTMLParagraphElement,
  React.HTMLAttributes<HTMLParagraphElement>
>(({ className, ...props }, ref) => (
  <p
    ref={ref}
    className={cn("text-xs text-muted-foreground leading-relaxed mt-[4px]", className)}
    {...props}
  />
))
FormDescription.displayName = "FormDescription"

const FormMessage = React.forwardRef<
  HTMLParagraphElement,
  React.HTMLAttributes<HTMLParagraphElement>
>(({ className, children, ...props }, ref) => {
  if (!children) return null
  return (
    <p
      ref={ref}
      className={cn("text-xs font-mono text-destructive mt-1", className)}
      {...props}
    >
      {children}
    </p>
  )
})
FormMessage.displayName = "FormMessage"

const FormError = React.forwardRef<
  HTMLDivElement,
  React.HTMLAttributes<HTMLDivElement>
>(({ className, ...props }, ref) => (
  <div
    ref={ref}
    className={cn(
      "border border-destructive/20 rounded-none bg-destructive/10 text-[#9a5e12] px-[10px] py-[8px] text-[13px] mb-[12px]",
      className
    )}
    {...props}
  />
))
FormError.displayName = "FormError"

export { FormField, FormLabel, FormDescription, FormMessage, FormError }
