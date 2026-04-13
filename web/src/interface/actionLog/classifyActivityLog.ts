import type { LucideIcon } from 'lucide-react'
import {
  Activity,
  AlertCircle,
  ArrowUpCircle,
  BadgeCheck,
  CircleCheck,
  ClipboardList,
  FolderPlus,
  ListPlus,
  MessageSquare,
  Package,
  PauseCircle,
  PenLine,
  RotateCw,
  ScrollText,
  ShieldAlert,
  SkipForward,
  Sparkles,
  Users,
  Wallet,
  XCircle,
} from 'lucide-react'

export type ActivityLogPresentation = {
  badge: string
  Icon: LucideIcon
  iconWrapClass: string
  badgeClass: string
  cardClass: string
}

const defaultPresentation: ActivityLogPresentation = {
  badge: 'Activity',
  Icon: Activity,
  iconWrapClass: 'bg-zinc-100 text-zinc-600',
  badgeClass: 'bg-zinc-100 text-zinc-600 border-zinc-200/80',
  cardClass: 'border-zinc-100 bg-white',
}

/**
 * Derives icon, badge, and tint from free-form `action` strings (no structured kind on entries).
 * Order matters: first match wins.
 */
export function classifyActivityLogAction(action: string): ActivityLogPresentation {
  const a = action.trim()

  if (/^proposed task:/i.test(a)) {
    return {
      badge: 'Proposed',
      Icon: ListPlus,
      iconWrapClass: 'bg-sky-50 text-sky-600',
      badgeClass: 'bg-sky-50 text-sky-700 border-sky-200/80',
      cardClass: 'border-sky-100/90 bg-sky-50/35',
    }
  }
  if (a === 'completed task' || /^completed task\b/i.test(a)) {
    return {
      badge: 'Done',
      Icon: CircleCheck,
      iconWrapClass: 'bg-emerald-50 text-emerald-600',
      badgeClass: 'bg-emerald-50 text-emerald-800 border-emerald-200/80',
      cardClass: 'border-emerald-100/90 bg-emerald-50/30',
    }
  }
  if (/defined project brief/i.test(a)) {
    return {
      badge: 'Brief',
      Icon: ScrollText,
      iconWrapClass: 'bg-violet-50 text-violet-600',
      badgeClass: 'bg-violet-50 text-violet-800 border-violet-200/80',
      cardClass: 'border-violet-100/90 bg-violet-50/25',
    }
  }
  if (/delivered final project/i.test(a)) {
    return {
      badge: 'Delivered',
      Icon: Package,
      iconWrapClass: 'bg-amber-50 text-amber-700',
      badgeClass: 'bg-amber-50 text-amber-900 border-amber-200/80',
      cardClass: 'border-amber-100/90 bg-amber-50/25',
    }
  }
  if (/approved task submission/i.test(a)) {
    return {
      badge: 'Approved',
      Icon: BadgeCheck,
      iconWrapClass: 'bg-emerald-50 text-emerald-600',
      badgeClass: 'bg-emerald-50 text-emerald-800 border-emerald-200/80',
      cardClass: 'border-emerald-100/90 bg-emerald-50/30',
    }
  }
  if (/requested changes on task/i.test(a)) {
    return {
      badge: 'Changes',
      Icon: MessageSquare,
      iconWrapClass: 'bg-orange-50 text-orange-600',
      badgeClass: 'bg-orange-50 text-orange-800 border-orange-200/80',
      cardClass: 'border-orange-100/90 bg-orange-50/25',
    }
  }
  if (/marked task on hold/i.test(a)) {
    return {
      badge: 'On hold',
      Icon: PauseCircle,
      iconWrapClass: 'bg-amber-50 text-amber-700',
      badgeClass: 'bg-amber-50 text-amber-900 border-amber-200/80',
      cardClass: 'border-amber-100/90 bg-amber-50/25',
    }
  }
  if (/promoted from backlog/i.test(a)) {
    return {
      badge: 'Promoted',
      Icon: ArrowUpCircle,
      iconWrapClass: 'bg-indigo-50 text-indigo-600',
      badgeClass: 'bg-indigo-50 text-indigo-800 border-indigo-200/80',
      cardClass: 'border-indigo-100/90 bg-indigo-50/25',
    }
  }
  if (/saved team template/i.test(a)) {
    return {
      badge: 'Team',
      Icon: Users,
      iconWrapClass: 'bg-fuchsia-50 text-fuchsia-600',
      badgeClass: 'bg-fuchsia-50 text-fuchsia-800 border-fuchsia-200/80',
      cardClass: 'border-fuchsia-100/90 bg-fuchsia-50/20',
    }
  }
  if (/^created project/i.test(a)) {
    return {
      badge: 'Project',
      Icon: FolderPlus,
      iconWrapClass: 'bg-slate-100 text-slate-700',
      badgeClass: 'bg-slate-100 text-slate-800 border-slate-200/80',
      cardClass: 'border-slate-100/90 bg-slate-50/50',
    }
  }
  if (/^Task failed:/i.test(a)) {
    return {
      badge: 'Failed',
      Icon: XCircle,
      iconWrapClass: 'bg-red-50 text-red-600',
      badgeClass: 'bg-red-50 text-red-800 border-red-200/80',
      cardClass: 'border-red-100/90 bg-red-50/25',
    }
  }
  if (/Retry queued/i.test(a)) {
    return {
      badge: 'Retry',
      Icon: RotateCw,
      iconWrapClass: 'bg-blue-50 text-blue-600',
      badgeClass: 'bg-blue-50 text-blue-800 border-blue-200/80',
      cardClass: 'border-blue-100/90 bg-blue-50/25',
    }
  }
  if (/^Requeue all:/i.test(a)) {
    return {
      badge: 'Requeue',
      Icon: ClipboardList,
      iconWrapClass: 'bg-blue-50 text-blue-600',
      badgeClass: 'bg-blue-50 text-blue-800 border-blue-200/80',
      cardClass: 'border-blue-100/90 bg-blue-50/25',
    }
  }
  if (/^Generating final/i.test(a)) {
    return {
      badge: 'Generating',
      Icon: Sparkles,
      iconWrapClass: 'bg-violet-50 text-violet-600',
      badgeClass: 'bg-violet-50 text-violet-800 border-violet-200/80',
      cardClass: 'border-violet-100/90 bg-violet-50/25',
    }
  }
  if (/^Error generating final/i.test(a)) {
    return {
      badge: 'Error',
      Icon: AlertCircle,
      iconWrapClass: 'bg-red-50 text-red-600',
      badgeClass: 'bg-red-50 text-red-800 border-red-200/80',
      cardClass: 'border-red-100/90 bg-red-50/25',
    }
  }
  if (/Blocked: project budget/i.test(a)) {
    return {
      badge: 'Budget',
      Icon: Wallet,
      iconWrapClass: 'bg-rose-50 text-rose-600',
      badgeClass: 'bg-rose-50 text-rose-800 border-rose-200/80',
      cardClass: 'border-rose-100/90 bg-rose-50/25',
    }
  }
  if (/User skipped final|User accepted mock/i.test(a)) {
    return {
      badge: 'Delivery',
      Icon: SkipForward,
      iconWrapClass: 'bg-teal-50 text-teal-600',
      badgeClass: 'bg-teal-50 text-teal-800 border-teal-200/80',
      cardClass: 'border-teal-100/90 bg-teal-50/20',
    }
  }
  if (/Delivery paused/i.test(a)) {
    return {
      badge: 'Paused',
      Icon: PauseCircle,
      iconWrapClass: 'bg-teal-50 text-teal-600',
      badgeClass: 'bg-teal-50 text-teal-800 border-teal-200/80',
      cardClass: 'border-teal-100/90 bg-teal-50/20',
    }
  }
  if (/User requested amend|revised task/i.test(a)) {
    return {
      badge: 'Replan',
      Icon: PenLine,
      iconWrapClass: 'bg-sky-50 text-sky-600',
      badgeClass: 'bg-sky-50 text-sky-800 border-sky-200/80',
      cardClass: 'border-sky-100/90 bg-sky-50/25',
    }
  }
  if (/Project reopened for iteration/i.test(a)) {
    return {
      badge: 'Iterate',
      Icon: RotateCw,
      iconWrapClass: 'bg-cyan-50 text-cyan-600',
      badgeClass: 'bg-cyan-50 text-cyan-800 border-cyan-200/80',
      cardClass: 'border-cyan-100/90 bg-cyan-50/20',
    }
  }
  if (/Blocked:.*LLM|cloud LLM call skipped/i.test(a)) {
    return {
      badge: 'Blocked',
      Icon: ShieldAlert,
      iconWrapClass: 'bg-rose-50 text-rose-600',
      badgeClass: 'bg-rose-50 text-rose-800 border-rose-200/80',
      cardClass: 'border-rose-100/90 bg-rose-50/25',
    }
  }

  return defaultPresentation
}
