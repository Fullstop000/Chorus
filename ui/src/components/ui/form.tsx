/**
 * Form Components
 * 
 * Consistent form layout helpers that work with shadcn/ui
 * Replaces custom: form-group, form-label, modal-field, modal-field-hint
 */

import * as React from "react"
import { cn } from "@/lib/utils"
import { Label } from "./label"

interface FormFieldProps extends React.HTMLAttributes<HTMLDivElement> {
  children: React.ReactNode
}

const FormField = React.forwardRef<HTMLDivElement, FormFieldProps>(
  ({ className, children, ...props }, ref) => (
    <div
      ref={ref}
      className={cn("space-y-2", className)}
      {...props}
    >
      {children}
    </div>
  )
)
FormField.displayName = "FormField"

interface FormLabelProps extends React.ComponentPropsWithoutRef<typeof Label> {
  required?: boolean
}

const FormLabel = React.forwardRef<
  React.ElementRef<typeof Label>,
  FormLabelProps
>(({ className, children, required, ...props }, ref) => (
  <Label
    ref={ref}
    className={cn(
      "font-mono text-[11px] uppercase tracking-[0.08em] text-[var(--text-muted)]",
      className
    )}
    {...props}
  >
    {children}
    {required && <span className="text-[#c67a18] ml-1">*</span>}
  </Label>
))
FormLabel.displayName = "FormLabel"

interface FormDescriptionProps extends React.HTMLAttributes<HTMLParagraphElement> {
  children: React.ReactNode
}

const FormDescription = React.forwardRef<
  HTMLParagraphElement,
  FormDescriptionProps
>(({ className, children, ...props }, ref) => (
  <p
    ref={ref}
    className={cn("text-[12px] text-[var(--text-muted)] leading-relaxed", className)}
    {...props}
  >
    {children}
  </p>
))
FormDescription.displayName = "FormDescription"

interface FormMessageProps extends React.HTMLAttributes<HTMLParagraphElement> {
  children?: React.ReactNode
}

const FormMessage = React.forwardRef<
  HTMLParagraphElement,
  FormMessageProps
>(({ className, children, ...props }, ref) => {
  if (!children) return null
  return (
    <p
      ref={ref}
      className={cn(
        "text-[12px] font-medium text-[#c67a18]",
        className
      )}
      {...props}
    >
      {children}
    </p>
  )
})
FormMessage.displayName = "FormMessage"

interface FormErrorProps extends React.HTMLAttributes<HTMLDivElement> {
  children: React.ReactNode
}

const FormError = React.forwardRef<HTMLDivElement, FormErrorProps>(
  ({ className, children, ...props }, ref) => (
    <div
      ref={ref}
      className={cn(
        "border border-[rgba(198,122,24,0.18)]",
        "bg-[rgba(198,122,24,0.08)]",
        "text-[#9a5e12]",
        "px-3 py-2 text-[13px]",
        className
      )}
      {...props}
    >
      {children}
    </div>
  )
)
FormError.displayName = "FormError"

export {
  FormField,
  FormLabel,
  FormDescription,
  FormMessage,
  FormError,
}
