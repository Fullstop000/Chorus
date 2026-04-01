import * as React from "react"
import { Slot } from "@radix-ui/react-slot"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"

const buttonVariants = cva(
  [
    "inline-flex items-center justify-center gap-2",
    "font-mono text-xs uppercase tracking-[0.05em]",
    "border border-transparent rounded-none",
    "transition-[background,color,border-color] duration-150 ease-in-out",
    "active:translate-y-px",
    "disabled:opacity-45 disabled:pointer-events-none",
    "cursor-pointer",
  ],
  {
    variants: {
      variant: {
        default: [
          "border-input bg-popover text-foreground",
          "hover:bg-primary hover:text-primary-foreground hover:border-primary",
        ],
        destructive: [
          "border-destructive bg-destructive text-destructive-foreground",
          "hover:bg-destructive/90",
        ],
        outline: [
          "border-input bg-transparent text-foreground",
          "hover:bg-accent hover:text-accent-foreground",
        ],
        ghost: [
          "border-transparent bg-transparent text-foreground",
          "hover:bg-accent hover:text-accent-foreground",
        ],
        link: [
          "border-transparent bg-transparent text-foreground underline-offset-4",
          "hover:underline",
        ],
      },
      size: {
        default: "min-h-9 px-3",
        sm: "min-h-[30px] px-2.5 text-[11px]",
        lg: "min-h-[42px] px-4",
        icon: "h-9 w-9 shrink-0",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button"
    return (
      <Comp
        className={cn(buttonVariants({ variant, size, className }))}
        ref={ref}
        {...props}
      />
    )
  }
)
Button.displayName = "Button"

export { Button, buttonVariants }
