import { useEffect, useState, type ReactNode } from "react"
import {
  AlertTriangle, Check, ChevronLeft, ChevronRight, CircleAlert, Eye, Loader2,
  RefreshCw, RotateCcw, Search, Sparkles, User as UserIcon,
} from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Combobox, type ComboboxOption } from "@/components/ui/combobox"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip"
import { useI18n } from "@/lib/i18n"

/**
 * Shared layout and interaction pieces for all Review queues. Every queue uses the same header,
 * filters, status language, pagination, provenance block, and right-side review sheet.
 */

export const REVIEW_PAGE_SIZE = 20
export type ReviewFilter = "all" | "pending" | "decided"
export type ReviewStatusTone = "pending" | "warning" | "error" | "success" | "neutral"

const ALL_DECISION_MAKERS = "__all_decision_makers__"

export function isAgentDecisionMaker(by: string | null): boolean {
  return Boolean(by && (by === "agent" || by.endsWith("-agent")))
}

function reviewTimestamp(iso: string): number {
  const timestamp = /(?:Z|[+-]\d{2}:\d{2})$/i.test(iso) ? iso : `${iso}Z`
  return new Date(timestamp).getTime()
}

/** Match the provenance fields shown in a Review row against the shared filters. */
export function matchesReviewFilters({
  when, by, startDate, endDate, decisionMaker,
}: {
  when: string | null
  by: string | null
  startDate: string
  endDate: string
  decisionMaker: string | null
}): boolean {
  if (decisionMaker) {
    if (decisionMaker === "agent") {
      if (!isAgentDecisionMaker(by)) return false
    } else if (by?.toLocaleLowerCase() !== decisionMaker.toLocaleLowerCase()) {
      return false
    }
  }
  if (!startDate && !endDate) return true
  if (!when) return false
  const value = reviewTimestamp(when)
  if (Number.isNaN(value)) return false
  if (startDate && value < new Date(`${startDate}T00:00:00`).getTime()) return false
  if (endDate && value > new Date(`${endDate}T23:59:59.999`).getTime()) return false
  return true
}

export function ReviewQueueHeader({
  title, query, onQueryChange, filter, onFilterChange, pendingCount,
  startDate, onStartDateChange, endDate, onEndDateChange,
  decisionMaker, onDecisionMakerChange, decisionMakers,
  onReset, onRefresh, refreshing = false, summary,
}: {
  title: string
  query: string
  onQueryChange: (value: string) => void
  filter: ReviewFilter
  onFilterChange: (value: ReviewFilter) => void
  pendingCount: number
  startDate: string
  onStartDateChange: (value: string) => void
  endDate: string
  onEndDateChange: (value: string) => void
  decisionMaker: string | null
  onDecisionMakerChange: (value: string | null) => void
  decisionMakers: string[]
  onReset: () => void
  onRefresh: () => void
  refreshing?: boolean
  summary?: ReactNode
}) {
  const { t } = useI18n()
  const makerOptions: ComboboxOption[] = [
    { value: ALL_DECISION_MAKERS, label: t("review.allDecisionMakers") },
    { value: "agent", label: t("common.agent") },
    ...[...new Set(decisionMakers)]
      .filter((name) => name && !isAgentDecisionMaker(name))
      .sort((left, right) => left.localeCompare(right))
      .map((name) => ({ value: name, label: name })),
  ]
  const hasFilters = Boolean(query || filter !== "all" || startDate || endDate || decisionMaker)
  return (
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div className="min-w-0 max-w-2xl">
        <h2 className="text-sm font-semibold">{title}</h2>
      </div>
      <div className="flex flex-wrap items-center justify-end gap-2">
        {summary}
        <div className="relative">
          <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(event) => onQueryChange(event.target.value)}
            placeholder={t("common.search")}
            className="h-8 w-48 pl-7 text-sm"
          />
        </div>
        <Select value={filter} onValueChange={(value) => onFilterChange(value as ReviewFilter)}>
          <SelectTrigger className="h-8 w-32 text-sm"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t("review.statusAll")}</SelectItem>
            <SelectItem value="pending">{t("common.pending")}{pendingCount ? ` (${pendingCount})` : ""}</SelectItem>
            <SelectItem value="decided">{t("common.decided")}</SelectItem>
          </SelectContent>
        </Select>
        <div className="flex items-center gap-1" title={t("review.timeRange")}>
          <span className="mr-1 whitespace-nowrap text-xs text-muted-foreground">{t("review.timeRange")}</span>
          <Input
            type="date"
            value={startDate}
            max={endDate || undefined}
            onChange={(event) => onStartDateChange(event.target.value)}
            aria-label={t("review.startDate")}
            title={t("review.startDate")}
            className="h-8 w-[8.8rem] text-sm"
          />
          <span className="text-xs text-muted-foreground">–</span>
          <Input
            type="date"
            value={endDate}
            min={startDate || undefined}
            onChange={(event) => onEndDateChange(event.target.value)}
            aria-label={t("review.endDate")}
            title={t("review.endDate")}
            className="h-8 w-[8.8rem] text-sm"
          />
        </div>
        <Combobox
          value={decisionMaker ?? ALL_DECISION_MAKERS}
          onChange={(value) => onDecisionMakerChange(value === ALL_DECISION_MAKERS ? null : value)}
          options={makerOptions}
          placeholder={t("review.decisionMaker")}
          searchPlaceholder={t("review.searchUsers")}
          emptyText={t("review.noUsers")}
          className="w-44"
          triggerClassName="h-8"
        />
        <Button
          size="sm"
          variant="outline"
          className="h-8 gap-1.5"
          onClick={onReset}
          disabled={!hasFilters}
        >
          <RotateCcw className="h-3.5 w-3.5" /> {t("common.reset")}
        </Button>
        <Button
          size="icon"
          variant="outline"
          className="h-8 w-8"
          onClick={onRefresh}
          disabled={refreshing}
          title={t("common.refresh")}
        >
          <RefreshCw className={`h-3.5 w-3.5 ${refreshing ? "animate-spin" : ""}`} />
        </Button>
      </div>
    </div>
  )
}

export function ReviewTableFrame({ children }: { children: ReactNode }) {
  return <div className="overflow-x-auto rounded-lg border bg-card/20">{children}</div>
}

export function ReviewPagination({
  page, pageSize = REVIEW_PAGE_SIZE, total, onPageChange,
}: {
  page: number
  pageSize?: number
  total: number
  onPageChange: (page: number) => void
}) {
  const { t } = useI18n()
  if (total <= pageSize) return null
  const pageCount = Math.max(1, Math.ceil(total / pageSize))
  const safePage = Math.min(page, pageCount - 1)
  return (
    <div className="flex items-center justify-between text-xs text-muted-foreground">
      <span>{t("review.page", {
        start: safePage * pageSize + 1,
        end: Math.min(total, (safePage + 1) * pageSize),
        total,
      })}</span>
      <div className="flex gap-1">
        <Button size="icon" variant="outline" className="h-7 w-7" disabled={safePage === 0} onClick={() => onPageChange(safePage - 1)}>
          <ChevronLeft className="h-4 w-4" />
        </Button>
        <Button size="icon" variant="outline" className="h-7 w-7" disabled={safePage >= pageCount - 1} onClick={() => onPageChange(safePage + 1)}>
          <ChevronRight className="h-4 w-4" />
        </Button>
      </div>
    </div>
  )
}

export function ReviewStatusBadge({
  tone, children, title,
}: {
  tone: ReviewStatusTone
  children: ReactNode
  title?: string
}) {
  const icon = tone === "error" ? <CircleAlert className="h-3 w-3" />
    : tone === "pending" || tone === "warning" ? <AlertTriangle className="h-3 w-3" />
      : tone === "success" ? <Check className="h-3 w-3" /> : null
  const classes = tone === "error"
    ? "border-destructive/30 bg-destructive/10 text-destructive"
    : tone === "pending" || tone === "warning"
      ? "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-400"
      : tone === "success"
        ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"
        : "text-muted-foreground"
  return <Badge variant="outline" title={title} className={`gap-1 text-[10px] ${classes}`}>{icon}{children}</Badge>
}

export function ReviewActionButton({ onClick, disabled = false }: { onClick: () => void; disabled?: boolean }) {
  const { t } = useI18n()
  return (
    <Button size="sm" variant="outline" className="h-7 gap-1" onClick={onClick} disabled={disabled}>
      <Eye className="h-3.5 w-3.5" /> {t("review.review")}
    </Button>
  )
}

export function ReviewProvenance({ by, when, meta }: { by: string | null; when: string | null; meta?: ReactNode }) {
  return (
    <div className="space-y-1.5">
      <By by={by} />
      {meta}
      <p className="whitespace-nowrap text-[11px] text-muted-foreground" title={when ?? ""}>{fmtWhen(when)}</p>
    </div>
  )
}

export function ReviewDetailSheet({
  open, onOpenChange, badges, title, description, children, footer,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  badges?: ReactNode
  title: ReactNode
  description?: ReactNode
  children: ReactNode
  footer?: ReactNode
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[88vh] w-[min(1080px,calc(100vw-2rem))] max-w-none flex-col gap-0 overflow-hidden border bg-background p-0 text-foreground shadow-2xl sm:max-w-none">
        <DialogHeader className="shrink-0 border-b px-6 py-5 pr-14">
          {badges && <div className="flex flex-wrap items-center gap-2">{badges}</div>}
          <DialogTitle className="mt-1 text-lg leading-snug">{title}</DialogTitle>
          {description && <DialogDescription className="max-w-4xl whitespace-pre-wrap leading-relaxed">{description}</DialogDescription>}
        </DialogHeader>
        <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 py-5">{children}</div>
        {footer && <div className="shrink-0 border-t bg-muted/20 px-6 py-4">{footer}</div>}
      </DialogContent>
    </Dialog>
  )
}

/** Compact local timestamp for Review tables; "—" when a row has no timestamp. */
export const fmtWhen = (iso: string | null) => {
  if (!iso) return "—"
  const date = new Date(reviewTimestamp(iso))
  if (Number.isNaN(date.getTime())) return "—"
  const pad = (value: number) => String(value).padStart(2, "0")
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

/** agent-vs-human provenance for a decided row; "—" when nobody has decided yet. */
export function By({ by }: { by: string | null }) {
  const { t } = useI18n()
  if (!by) return <span className="text-muted-foreground">—</span>
  const agent = isAgentDecisionMaker(by)
  return (
    <Badge variant="outline" className={`gap-1 text-[10px] ${agent ? "text-primary" : "text-muted-foreground"}`}>
      {agent ? <Sparkles className="h-3 w-3" /> : <UserIcon className="h-3 w-3" />}{agent ? t("common.agent") : by}
    </Badge>
  )
}

export const REASON_MAX = 200

/** Truncated text that reveals its FULL content in a tooltip on hover (for long reasons/details). */
export function HoverText({ text, className = "" }: { text: string; className?: string }) {
  return (
    <TooltipProvider delayDuration={150}>
      <Tooltip>
        <TooltipTrigger asChild>
          <span className={`block max-w-[22rem] cursor-default truncate ${className}`}>{text}</span>
        </TooltipTrigger>
        <TooltipContent className="max-w-sm whitespace-normal break-words leading-snug">{text}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}

/**
 * Inline-editable rationale cell. Click to edit (writers only); Enter/blur saves, Esc cancels. The
 * saved text is short (≤200) and feeds back into the owning agent's prompt as experience, so editing
 * it steers future auto-decisions.
 */
export function ReasonCell({
  value, fallback, canWrite, onSave,
}: {
  value: string | null
  fallback?: string
  canWrite: boolean
  onSave: (v: string) => Promise<void>
}) {
  const { t } = useI18n()
  const [editing, setEditing] = useState(false)
  const [text, setText] = useState(value ?? "")
  const [saving, setSaving] = useState(false)
  useEffect(() => { setText(value ?? "") }, [value])

  if (!editing) {
    if (!canWrite) return <span className="text-muted-foreground">{value || fallback || "—"}</span>
    return (
      <button
        type="button"
        className="block max-w-full whitespace-normal break-words text-left text-muted-foreground hover:text-foreground"
        title={value ? t("review.editReasonTitle", { reason: value }) : t("review.addReasonTitle")}
        onClick={() => setEditing(true)}
      >
        {value || fallback || <span className="italic opacity-60">{t("review.addReason")}</span>}
      </button>
    )
  }
  const commit = async () => {
    const next = text.trim().slice(0, REASON_MAX)
    if (next === (value ?? "")) { setEditing(false); return } // no change → no write / no audit noise
    setSaving(true)
    try { await onSave(next); setEditing(false) }
    finally { setSaving(false) }
  }
  return (
    <div className="flex items-center gap-1">
      <Input
        autoFocus value={text} maxLength={REASON_MAX}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit()
          if (e.key === "Escape") { setText(value ?? ""); setEditing(false) }
        }}
        onBlur={commit}
        className="h-7 text-xs"
      />
      <Button size="icon" variant="ghost" className="h-6 w-6 shrink-0" disabled={saving}
        onMouseDown={(e) => e.preventDefault()} onClick={commit}>
        {saving ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Check className="h-3.5 w-3.5" />}
      </Button>
    </div>
  )
}
