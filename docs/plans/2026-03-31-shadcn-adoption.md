# shadcn/ui Adoption Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Incrementally adopt shadcn/ui (Radix Primitives + React Hook Form) to improve accessibility and reduce custom CSS/form boilerplate while preserving the brutalist aesthetic.

**Architecture:** Replace custom UI primitives (modals, forms, dropdowns) with shadcn components built on Radix Primitives. Extract duplicated scroll visibility logic into a shared hook. Form validation uses React Hook Form with Zod schemas. No component library package dependency—shadcn components are copy-pasted and customized for brutalist style.

**Tech Stack:** shadcn/ui, Radix Primitives, React Hook Form, Zod

---

## Phase 1: Foundation Setup

### Task 1: Initialize shadcn/ui

**Files:**
- Modify: `ui/package.json`
- Create: `ui/components.json`
- Create: `ui/tsconfig.json` (update path aliases)

**Step 1: Add dependencies**

Run in `ui/`:
```bash
npm install class-variance-authority clsx tailwind-merge tailwindcss autoprefixer postcss
npm install @radix-ui/react-dialog @radix-ui/react-select @radix-ui/react-dropdown-menu @radix-ui/react-popover @radix-ui/react-checkbox @radix-ui/react-label @radix-ui/react-slot
npm install react-hook-form @hookform/resolvers zod
```

**Step 2: Initialize Tailwind**

Run:
```bash
npx tailwindcss init -p
```

**Step 3: Configure components.json**

```json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "default",
  "rsc": false,
  "tsx": true,
  "tailwind": {
    "config": "tailwind.config.js",
    "css": "src/index.css",
    "baseColor": "slate",
    "cssVariables": true
  },
  "aliases": {
    "components": "@/components",
    "utils": "@/lib/utils",
    "ui": "@/components/ui"
  }
}
```

**Step 4: Create lib/utils.ts**

Create `ui/src/lib/utils.ts`:
```typescript
import { type ClassValue, clsx } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
```

**Step 5: Update tsconfig.json path aliases**

Add to `ui/tsconfig.json`:
```json
"paths": {
  "@/*": ["./src/*"]
}
```

**Step 6: Commit**

```bash
git add ui/package.json ui/components.json ui/src/lib/utils.ts
git commit -m "feat(ui): initialize shadcn/ui foundation with Radix primitives"
```

---

### Task 2: Create Brutalist Theme Tailwind Config

**Files:**
- Modify: `ui/tailwind.config.js`

**Step 1: Create brutalist theme config**

Replace default tailwind config with:

```js
/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ["class"],
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
      },
      borderRadius: {
        // Brutalist: zero radius everywhere
        DEFAULT: "0px",
        lg: "0px",
        md: "0px",
        sm: "0px",
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
        mono: ["IBM Plex Mono", "monospace"],
      },
      boxShadow: {
        // Brutalist: no shadows
        DEFAULT: "none",
      },
    },
  },
  plugins: [],
}
```

**Step 2: Create index.css with CSS variables**

Create `ui/src/index.css`:

```css
@tailwind base;
@tailwind components;
@tailwind utilities;

@layer base {
  :root {
    --background: 40 20% 97%;
    --foreground: 220 10% 10%;
    --card: 40 20% 95%;
    --card-foreground: 220 10% 10%;
    --popover: 40 20% 97%;
    --popover-foreground: 220 10% 10%;
    --primary: 220 10% 15%;
    --primary-foreground: 40 20% 97%;
    --secondary: 40 10% 90%;
    --secondary-foreground: 220 10% 10%;
    --muted: 40 10% 90%;
    --muted-foreground: 220 5% 45%;
    --accent: 40 10% 90%;
    --accent-foreground: 220 10% 10%;
    --destructive: 0 70% 50%;
    --destructive-foreground: 40 20% 97%;
    --border: 220 10% 85%;
    --input: 220 10% 85%;
    --ring: 220 10% 15%;
  }
}

@layer base {
  * {
    @apply border-border;
  }
  body {
    @apply bg-background text-foreground;
  }
}
```

**Step 3: Import index.css in main.tsx**

Modify `ui/src/main.tsx`:
```typescript
import "./index.css"
```

**Step 4: Commit**

```bash
git add ui/tailwind.config.js ui/src/index.css ui/src/main.tsx
git commit -m "feat(ui): add brutalist Tailwind theme"
```

---

## Phase 2: Core UI Components

### Task 3: Add shadcn Button Component

**Files:**
- Create: `ui/src/components/ui/button.tsx`

**Step 1: Create button component**

```tsx
import * as React from "react"
import { Slot } from "@radix-ui/react-slot"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"

const buttonVariants = cva(
  "inline-flex items-center justify-center whitespace-nowrap text-sm font-mono uppercase tracking-wider transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary/90",
        destructive: "bg-destructive text-destructive-foreground hover:bg-destructive/90",
        outline: "border border-input bg-background hover:bg-accent hover:text-accent-foreground",
        secondary: "bg-secondary text-secondary-foreground hover:bg-secondary/80",
        ghost: "hover:bg-accent hover:text-accent-foreground",
        link: "underline-offset-4 hover:underline text-primary",
        brutal: "bg-foreground text-background border-2 border-foreground hover:bg-background hover:text-foreground",
      },
      size: {
        default: "h-9 px-4 py-2",
        sm: "h-8 px-3 text-xs",
        lg: "h-10 px-8",
        icon: "h-9 w-9",
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
```

**Step 2: Run build to verify**

Run: `cd ui && npm run build`
Expected: No errors

**Step 3: Commit**

```bash
git add ui/src/components/ui/button.tsx
git commit -m "feat(ui): add shadcn Button component with brutal variant"
```

---

### Task 4: Add shadcn Dialog Component

**Files:**
- Create: `ui/src/components/ui/dialog.tsx`

**Step 1: Create dialog components**

```tsx
import * as React from "react"
import * as DialogPrimitive from "@radix-ui/react-dialog"
import { X } from "lucide-react"
import { cn } from "@/lib/utils"

const Dialog = DialogPrimitive.Root
const DialogTrigger = DialogPrimitive.Trigger
const DialogPortal = DialogPrimitive.Portal
const DialogClose = DialogPrimitive.Close

const DialogOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Overlay
    ref={ref}
    className={cn(
      "fixed inset-0 z-50 bg-black/50 backdrop-blur-sm data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
      className
    )}
    {...props}
  />
))
DialogOverlay.displayName = DialogPrimitive.Overlay.displayName

const DialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content>
>(({ className, children, ...props }, ref) => (
  <DialogPortal>
    <DialogOverlay />
    <DialogPrimitive.Content
      ref={ref}
      className={cn(
        "fixed left-[50%] top-[50%] z-50 grid w-full max-w-lg translate-x-[-50%] translate-y-[-50%] gap-4 border-2 border-foreground bg-background p-6 shadow-none duration-200 data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[state=closed]:slide-out-to-left-1/2 data-[state=closed]:slide-out-to-top-[48%] data-[state=open]:slide-in-from-left-1/2 data-[state=open]:slide-in-from-top-[48%]",
        className
      )}
      {...props}
    >
      {children}
      <DialogPrimitive.Close className="absolute right-4 top-4 rounded-none border-2 border-foreground bg-background p-1 hover:bg-foreground hover:text-background transition-colors focus:outline-none focus:ring-1 focus:ring-ring">
        <X className="h-4 w-4" />
        <span className="sr-only">Close</span>
      </DialogPrimitive.Close>
    </DialogPrimitive.Content>
  </DialogPortal>
))
DialogContent.displayName = DialogPrimitive.Content.displayName

const DialogHeader = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn("flex flex-col space-y-1.5 text-center sm:text-left", className)}
    {...props}
  />
)
DialogHeader.displayName = "DialogHeader"

const DialogFooter = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn("flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2", className)}
    {...props}
  />
)
DialogFooter.displayName = "DialogFooter"

const DialogTitle = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Title>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Title
    ref={ref}
    className={cn("text-lg font-semibold leading-none tracking-tight font-mono uppercase", className)}
    {...props}
  />
))
DialogTitle.displayName = DialogPrimitive.Title.displayName

const DialogDescription = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Description>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Description
    ref={ref}
    className={cn("text-sm text-muted-foreground", className)}
    {...props}
  />
))
DialogDescription.displayName = DialogPrimitive.Description.displayName

export {
  Dialog,
  DialogPortal,
  DialogOverlay,
  DialogTrigger,
  DialogClose,
  DialogContent,
  DialogHeader,
  DialogFooter,
  DialogTitle,
  DialogDescription,
}
```

**Step 2: Run build to verify**

Run: `cd ui && npm run build`
Expected: No errors

**Step 3: Commit**

```bash
git add ui/src/components/ui/dialog.tsx
git commit -m "feat(ui): add shadcn Dialog component"
```

---

### Task 5: Add shadcn Form Components

**Files:**
- Create: `ui/src/components/ui/form.tsx`
- Create: `ui/src/components/ui/input.tsx`
- Create: `ui/src/components/ui/label.tsx`

**Step 1: Create form.tsx**

```tsx
import * as React from "react"
import type * as LabelPrimitive from "@radix-ui/react-label"
import { Slot } from "@radix-ui/react-slot"
import {
  Controller,
  ControllerProps,
  FieldPath,
  FieldValues,
  FormProvider,
  useFormContext,
} from "react-hook-form"
import { cn } from "@/lib/utils"
import { Label } from "@/components/ui/label"

const Form = FormProvider

type FormFieldContextValue<
  TFieldValues extends FieldValues = FieldValues,
  TName extends FieldPath<TFieldValues> = FieldPath<TFieldValues>
> = {
  name: TName
}

const FormFieldContext = React.createContext<FormFieldContextValue>({} as FormFieldContextValue)

const FormField = <
  TFieldValues extends FieldValues = FieldValues,
  TName extends FieldPath<TFieldValues> = FieldPath<TFieldValues>
>({
  ...props
}: ControllerProps<TFieldValues, TName>) => {
  return (
    <FormFieldContext.Provider value={{ name: props.name }}>
      <Controller {...props} />
    </FormFieldContext.Provider>
  )
}

const useFormField = () => {
  const fieldContext = React.useContext(FormFieldContext)
  const itemContext = React.useContext(FormItemContext)
  const { getFieldState, formState } = useFormContext()

  const fieldState = getFieldState(fieldContext.name, formState)

  if (!fieldContext) {
    throw new Error("useFormField should be used within <FormField>")
  }

  const { id } = itemContext

  return {
    id,
    name: fieldContext.name,
    formItemId: `${id}-form-item`,
    formDescriptionId: `${id}-form-item-description`,
    formMessageId: `${id}-form-item-message`,
    ...fieldState,
  }
}

type FormItemContextValue = {
  id: string
}

const FormItemContext = React.createContext<FormItemContextValue>({} as FormItemContextValue)

const FormItem = React.forwardRef<
  HTMLDivElement,
  React.HTMLAttributes<HTMLDivElement>
>(({ className, ...props }, ref) => {
  const id = React.useId()

  return (
    <FormItemContext.Provider value={{ id }}>
      <div ref={ref} className={cn("space-y-2", className)} {...props} />
    </FormItemContext.Provider>
  )
})
FormItem.displayName = "FormItem"

const FormLabel = React.forwardRef<
  React.ElementRef<typeof LabelPrimitive.Root>,
  React.ComponentPropsWithoutRef<typeof LabelPrimitive.Root>
>(({ className, ...props }, ref) => {
  const { error, formItemId } = useFormField()

  return (
    <Label
      ref={ref}
      className={cn(error && "text-destructive", className)}
      htmlFor={formItemId}
      {...props}
    />
  )
})
FormLabel.displayName = "FormLabel"

const FormControl = React.forwardRef<
  React.ElementRef<typeof Slot>,
  React.ComponentPropsWithoutRef<typeof Slot>
>(({ ...props }, ref) => {
  const { error, formItemId, formDescriptionId, formMessageId } = useFormField()

  return (
    <Slot
      ref={ref}
      id={formItemId}
      aria-describedby={!error ? formDescriptionId : `${formDescriptionId} ${formMessageId}`}
      aria-invalid={!!error}
      {...props}
    />
  )
})
FormControl.displayName = "FormControl"

const FormDescription = React.forwardRef<
  HTMLParagraphElement,
  React.HTMLAttributes<HTMLParagraphElement>
>(({ className, ...props }, ref) => {
  const { formDescriptionId } = useFormField()

  return (
    <p
      ref={ref}
      id={formDescriptionId}
      className={cn("text-xs text-muted-foreground", className)}
      {...props}
    />
  )
})
FormDescription.displayName = "FormDescription"

const FormMessage = React.forwardRef<
  HTMLParagraphElement,
  React.HTMLAttributes<HTMLParagraphElement>
>(({ className, children, ...props }, ref) => {
  const { error, formMessageId } = useFormField()
  const body = error ? String(error?.message) : children

  if (!body) {
    return null
  }

  return (
    <p
      ref={ref}
      id={formMessageId}
      className={cn("text-xs font-mono text-destructive", className)}
      {...props}
    >
      {body}
    </p>
  )
})
FormMessage.displayName = "FormMessage"

export {
  useFormField,
  Form,
  FormItem,
  FormLabel,
  FormControl,
  FormDescription,
  FormMessage,
  FormField,
}
```

**Step 2: Create input.tsx**

```tsx
import * as React from "react"
import { cn } from "@/lib/utils"

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {}

const Input = React.forwardRef<HTMLInputElement, InputProps>(
  ({ className, type, ...props }, ref) => {
    return (
      <input
        type={type}
        className={cn(
          "flex h-9 w-full border-2 border-foreground bg-transparent px-3 py-1 text-sm shadow-none transition-colors file:border-0 file:bg-transparent file:text-sm file:font-medium focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 font-mono",
          className
        )}
        ref={ref}
        {...props}
      />
    )
  }
)
Input.displayName = "Input"

export { Input }
```

**Step 3: Create label.tsx**

```tsx
import * as React from "react"
import * as LabelPrimitive from "@radix-ui/react-label"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"

const labelVariants = cva(
  "text-xs font-mono uppercase tracking-wider text-foreground"
)

const Label = React.forwardRef<
  React.ElementRef<typeof LabelPrimitive.Root>,
  React.ComponentPropsWithoutRef<typeof LabelPrimitive.Root> & VariantProps<typeof labelVariants>
>(({ className, ...props }, ref) => (
  <LabelPrimitive.Root ref={ref} className={cn(labelVariants(), className)} {...props} />
))
Label.displayName = LabelPrimitive.Root.displayName

export { Label }
```

**Step 4: Run build to verify**

Run: `cd ui && npm run build`
Expected: No errors

**Step 5: Commit**

```bash
git add ui/src/components/ui/form.tsx ui/src/components/ui/input.tsx ui/src/components/ui/label.tsx
git commit -m "feat(ui): add shadcn Form, Input, Label components"
```

---

## Phase 3: Extract Shared Hooks

### Task 6: Extract useVisibilityTracking Hook

**Files:**
- Create: `ui/src/hooks/useVisibilityTracking.ts`

**Step 1: Create the hook**

```typescript
import { useCallback, useRef, useState } from "react"

interface VisibilityItem {
  seq: number
  id: string
  element: HTMLElement | null
}

export function useVisibilityTracking(getItemKey: (seq: number) => string) {
  const [highestVisibleSeq, setHighestVisibleSeq] = useState<number>(0)
  const pendingReadsRef = useRef<Map<string, number>>(new Map())
  const rafRef = useRef<number | null>(null)

  const collectHighestVisibleSeq = useCallback(() => {
    const items: VisibilityItem[] = []
    pendingReadsRef.current.forEach((seq, id) => {
      const element = document.getElementById(id)
      if (element) {
        items.push({ seq, id, element })
      }
    })

    let maxSeq = highestVisibleSeq
    for (const item of items) {
      const rect = item.element!.getBoundingClientRect()
      const isVisible = rect.top < window.innerHeight && rect.bottom > 0
      if (isVisible && item.seq > maxSeq) {
        maxSeq = item.seq
      }
    }

    if (maxSeq > highestVisibleSeq) {
      setHighestVisibleSeq(maxSeq)
    }

    pendingReadsRef.current.clear()
  }, [highestVisibleSeq])

  const scheduleVisibilityCheck = useCallback(
    (seq: number, id: string) => {
      pendingReadsRef.current.set(id, seq)

      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current)
      }
      rafRef.current = requestAnimationFrame(collectHighestVisibleSeq)
    },
    [collectHighestVisibleSeq]
  )

  const scheduleInitialVisibilityRead = useCallback(
    (seq: number) => {
      const id = getItemKey(seq)
      scheduleVisibilityCheck(seq, id)
    },
    [getItemKey, scheduleVisibilityCheck]
  )

  return {
    highestVisibleSeq,
    scheduleInitialVisibilityRead,
    scheduleVisibilityCheck,
  }
}
```

**Step 2: Verify TypeScript compilation**

Run: `cd ui && npx tsc --noEmit`
Expected: No errors

**Step 3: Commit**

```bash
git add ui/src/hooks/useVisibilityTracking.ts
git commit -m "feat(ui): extract useVisibilityTracking hook"
```

---

## Phase 4: Migrate Existing Components

### Task 7: Migrate CreateChannelModal to Dialog + React Hook Form

**Files:**
- Modify: `ui/src/components/Channels/CreateChannelModal.tsx`
- Modify: `ui/src/components/Channels/CreateChannelModal.css`

**Step 1: Read current implementation**

Read `ui/src/components/Channels/CreateChannelModal.tsx` to understand current form structure.

**Step 2: Rewrite with shadcn Dialog + React Hook Form**

Replace current implementation to use:
- `Dialog` component instead of custom modal
- `Form` components (FormField, FormLabel, FormControl, FormMessage)
- `Input` component instead of custom input
- React Hook Form with Zod schema

**Step 3: Run build and verify**

Run: `cd ui && npm run build`
Expected: No errors

**Step 4: Verify in browser**

Run: `./dev.sh` and verify the Create Channel modal works.

**Step 5: Commit**

```bash
git add ui/src/components/Channels/CreateChannelModal.tsx ui/src/components/Channels/CreateChannelModal.css
git commit -m "refactor(ui): migrate CreateChannelModal to shadcn Dialog + React Hook Form"
```

---

### Task 8: Migrate EditChannelModal to Dialog + React Hook Form

**Files:**
- Modify: `ui/src/components/Channels/EditChannelModal.tsx`
- Modify: `ui/src/components/Channels/EditChannelModal.css`

**Step 1: Read current implementation**

Read `ui/src/components/Channels/EditChannelModal.tsx` to understand current form structure.

**Step 2: Rewrite with shadcn Dialog + React Hook Form**

Use same pattern as CreateChannelModal migration.

**Step 3: Run build and verify**

Run: `cd ui && npm run build`
Expected: No errors

**Step 4: Commit**

```bash
git add ui/src/components/Channels/EditChannelModal.tsx ui/src/components/Channels/EditChannelModal.css
git commit -m "refactor(ui): migrate EditChannelModal to shadcn Dialog + React Hook Form"
```

---

### Task 9: Refactor ChatPanel to use useVisibilityTracking

**Files:**
- Modify: `ui/src/components/Chat/ChatPanel.tsx`

**Step 1: Read current implementation**

Read `ui/src/components/Chat/ChatPanel.tsx` to understand scroll visibility logic.

**Step 2: Import and use the hook**

Replace duplicated scroll visibility code with:
```typescript
import { useVisibilityTracking } from "@/hooks/useVisibilityTracking"

const getMessageKey = (seq: number) => `msg-${seq}`

// Replace the 60+ lines of scroll tracking code with:
const { highestVisibleSeq, scheduleInitialVisibilityRead } = useVisibilityTracking(getMessageKey)
```

**Step 3: Run build and verify**

Run: `cd ui && npm run build`
Expected: No errors

**Step 4: Verify in browser**

Run `./dev.sh` and verify message scrolling still works correctly.

**Step 5: Commit**

```bash
git add ui/src/components/Chat/ChatPanel.tsx
git commit -m "refactor(ui): refactor ChatPanel to use useVisibilityTracking hook"
```

---

### Task 10: Refactor ThreadPanel to use useVisibilityTracking

**Files:**
- Modify: `ui/src/components/Chat/ThreadPanel.tsx`

**Step 1: Read current implementation**

Read `ui/src/components/Chat/ThreadPanel.tsx` to understand scroll visibility logic.

**Step 2: Import and use the hook**

Same pattern as ChatPanel migration.

**Step 3: Run build and verify**

Run: `cd ui && npm run build`
Expected: No errors

**Step 4: Commit**

```bash
git add ui/src/components/Chat/ThreadPanel.tsx
git commit -m "refactor(ui): refactor ThreadPanel to use useVisibilityTracking hook"
```

---

## Phase 5: Continue Migration (Pick One)

At this point, decide which component to migrate next based on pain points. Options:

### Option A: Migrate AgentConfigForm

**Files:**
- Modify: `ui/src/components/Agents/AgentConfigForm.tsx`
- Complexity: High (276 lines, multiple choice cards, runtime selection)

### Option B: Add Select/Dropdown Components

**Files:**
- Create: `ui/src/components/ui/select.tsx`
- Create: `ui/src/components/ui/dropdown-menu.tsx`
- Then migrate sidebar channel selector

### Option C: Add Toast Component

**Files:**
- Create: `ui/src/components/ui/toast.tsx` (or use sonner)
- Then migrate ToastRegion.tsx

---

## Verification

After each phase:

1. Run `cd ui && npm run build` - must pass
2. Run `cd ui && npm run test` - must pass
3. Run end-to-end tests: `cargo test --test e2e_tests`
4. Manual browser verification via `./dev.sh`

---

## File Structure After Adoption

```
ui/src/
├── components/
│   ├── ui/                    # shadcn components
│   │   ├── button.tsx
│   │   ├── dialog.tsx
│   │   ├── form.tsx
│   │   ├── input.tsx
│   │   ├── label.tsx
│   │   └── select.tsx        # future
│   ├── chat/
│   │   ├── ChatPanel.tsx
│   │   └── ThreadPanel.tsx
│   ├── agents/
│   │   └── AgentConfigForm.tsx
│   └── channels/
│       ├── CreateChannelModal.tsx
│       └── EditChannelModal.tsx
├── hooks/
│   └── useVisibilityTracking.ts
└── lib/
    └── utils.ts
```
