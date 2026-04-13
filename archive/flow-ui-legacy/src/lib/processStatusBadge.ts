import type { badgeVariants } from "@/components/ui/badge";
import type { VariantProps } from "class-variance-authority";

export function processStatusBadgeVariant(
  status: string,
): NonNullable<VariantProps<typeof badgeVariants>["variant"]> {
  switch (status) {
    case "failed":
      return "destructive";
    case "completed":
      return "secondary";
    case "running":
      return "default";
    case "cancelled":
      return "outline";
    case "approval_required":
    case "task_review_required":
      return "outline";
    default:
      return "secondary";
  }
}
