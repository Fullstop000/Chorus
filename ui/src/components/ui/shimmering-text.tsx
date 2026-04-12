import type { HTMLAttributes } from "react";
import { cn } from "@/lib/cn";
import "./shimmering-text.css";

interface ShimmeringTextProps extends HTMLAttributes<HTMLSpanElement> {
  text: string;
}

export function ShimmeringText({
  text,
  className,
  ...props
}: ShimmeringTextProps) {
  return (
    <span className={cn("ui-shimmering-text", className)} {...props}>
      {text}
    </span>
  );
}
