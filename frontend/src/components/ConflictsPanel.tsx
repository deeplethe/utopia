import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { RotateCcw, Sparkles, Trash2 } from "lucide-react"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import { conflictSubject, conflictTypeLabel } from "@/lib/conflicts"
import type { Conflict, Reconciliation } from "@/lib/types"
import ConflictReviewSheet from "@/components/ConflictReviewSheet"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import {
  REVIEW_PAGE_SIZE, ReasonCell, ReviewActionButton, ReviewPagination, ReviewProvenance,
  ReviewQueueHeader, ReviewStatusBadge, ReviewTableFrame, type ReviewFilter,
  matchesReviewFilters,
} from "@/components/review-bits"

type Row = {
  key: string
  kind: "open" | "reconcile" | "duplicate"
  pending: boolean
  type: string
  subject: string
  detail: string | null
  handling: string
  severity?: Conflict["severity"]
  reason: string | null
  by: string | null
  when: string | null
  conflict?: Conflict // open rows carry the source conflict for the resolve menu
  onSaveReason?: (v: string) => Promise<void>
  onForget?: () => void
  onReopen?: () => void
}

/**
 * Conflicts — one table for the whole queue: open TBox conflicts (pending, on top) alongside the
 * decision memory those resolutions feed — domain/range reconciliation and duplicate-class calls.
 * A pending row acts via a compact "Resolve" menu (its resolutions + Dismiss); a decided row can
 * be forgotten (reconcile memory) or reopened (a duplicate pair), and its reason edited in place.
 */
export default function ConflictsPanel({
  ksId, conflicts, canWrite, onChanged,
}: {
  ksId: number
  conflicts: Conflict[]
  canWrite: boolean
  onChanged: () => void
}) {
  const { t } = useI18n()
  const confirmAction = useConfirm()
  const [recs, setRecs] = useState<Reconciliation[]>([])
  const [dups, setDups] = useState<Conflict[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<string | null>(null)
  const [selectedConflict, setSelectedConflict] = useState<Conflict | null>(null)
  const [q, setQ] = useState("")
  const [filter, setFilter] = useState<ReviewFilter>("all")
  const [startDate, setStartDate] = useState("")
  const [endDate, setEndDate] = useState("")
  const [decisionMaker, setDecisionMaker] = useState<string | null>(null)
  const [page, setPage] = useState(0)
  useEffect(() => { setPage(0) }, [q, filter, startDate, endDate, decisionMaker])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [r, d] = await Promise.all([
        api.listReconciliations(ksId, { limit: 500 }),
        api.listConflicts(ksId, "all", "duplicate"),
      ])
      setRecs(r.items)
      setDups(d.filter((c) => c.status !== "open")) // decided duplicates are the memory
    } catch (e) {
      toast.error(t("common.failedLoad", { error: (e as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [ksId, t])
  useEffect(() => { load() }, [load])

  const refresh = () => { load(); onChanged() }

  const resolve = async (c: Conflict, resolutionId: string) => {
    setBusy(`c${c.id}`)
    try {
      await api.resolveConflict(ksId, c.id, resolutionId)
      toast.success(t("review.conflictResolved"))
      setSelectedConflict(null)
      refresh()
    }
    catch (e) { toast.error(t("review.resolveFailed", { error: (e as Error).message.replace(/^\d+:\s*/, "") })) }
    finally { setBusy(null) }
  }
  const dismiss = async (c: Conflict) => {
    setBusy(`c${c.id}`)
    try {
      await api.dismissConflict(ksId, c.id)
      toast.success(t("review.dismissed"))
      setSelectedConflict(null)
      refresh()
    }
    catch (e) { toast.error(t("review.dismissFailed", { error: (e as Error).message.replace(/^\d+:\s*/, "") })) }
    finally { setBusy(null) }
  }
  const revokeRec = async (r: Reconciliation) => {
    const slot = r.slot === "domain" ? t("review.slot.domain") : t("review.slot.range")
    if (!await confirmAction(t("review.forgetReconciliation", { slot, name: r.property_label }), { destructive: true })) return
    setBusy(`r${r.id}`)
    try { await api.revokeReconciliation(ksId, r.id); toast.success(t("review.forgotten")); refresh() }
    catch (e) { toast.error(t("review.failed", { error: (e as Error).message.replace(/^\d+:\s*/, "") })) }
    finally { setBusy(null) }
  }
  const reopenDup = async (c: Conflict) => {
    if (!await confirmAction(t("review.reconsiderConfirm"))) return
    setBusy(`d${c.id}`)
    try { await api.reopenConflict(ksId, c.id); toast.success(t("review.reopened")); refresh() }
    catch (e) { toast.error(t("review.failed", { error: (e as Error).message.replace(/^\d+:\s*/, "") })) }
    finally { setBusy(null) }
  }

  const rows: Row[] = [
    ...conflicts.map<Row>((c) => ({
      key: `c${c.id}`, kind: "open", pending: true, type: conflictTypeLabel(c.ctype, t),
      subject: conflictSubject(c), detail: c.detail,
      handling: t("common.pending"), severity: c.severity,
      reason: c.payload.recommendation?.reason ?? null, by: null, when: c.created_at, conflict: c,
    })),
    ...recs.map<Row>((r) => ({
      key: `r${r.id}`, kind: "reconcile", pending: false,
      type: r.slot === "domain" ? t("review.slot.domain") : t("review.slot.range"),
      subject: r.property_label, detail: null,
      handling: (r.choice === "union" ? t("review.choice.union")
        : r.choice === "common_super" ? t("review.choice.commonSuper")
        : r.choice === "subsume" ? t("review.choice.subsume") : t("review.choice.keep"))
        + (r.chosen_label && r.choice !== "union" ? ` → ${r.chosen_label}` : ""),
      reason: r.reason, by: r.resolved_by, when: r.created_at,
      onForget: () => revokeRec(r),
      onSaveReason: async (v: string) => { await api.editReconciliationReason(ksId, r.id, v); refresh() },
    })),
    ...dups.map<Row>((c) => ({
      key: `d${c.id}`, kind: "duplicate", pending: false, type: t("review.duplicateClass"),
      subject: c.payload.entities.map((e) => e.label).join("  ·  "), detail: null,
      handling: c.status === "dismissed" ? t("review.keptDistinct") : t("review.merged"), reason: null,
      by: null, when: c.resolved_at ?? c.created_at, onReopen: () => reopenDup(c),
    })),
  ]

  const pendingCount = conflicts.length
  const decisionMakers = rows.flatMap((row) => row.by ? [row.by] : [])
  const term = q.trim().toLowerCase()
  const filtered = rows.filter((r) =>
    (filter === "all" || (filter === "pending" ? r.pending : !r.pending)) &&
    `${r.type} ${r.subject} ${r.handling} ${r.detail ?? ""}`.toLowerCase().includes(term) &&
    matchesReviewFilters({ when: r.when, by: r.by, startDate, endDate, decisionMaker }))
  const pageCount = Math.max(1, Math.ceil(filtered.length / REVIEW_PAGE_SIZE))
  const p = Math.min(page, pageCount - 1)
  const shown = filtered.slice(p * REVIEW_PAGE_SIZE, p * REVIEW_PAGE_SIZE + REVIEW_PAGE_SIZE)

  return (
    <div className="space-y-4">
      <ReviewQueueHeader
        title={t("review.conflicts.title")}
        query={q}
        onQueryChange={setQ}
        filter={filter}
        onFilterChange={setFilter}
        pendingCount={pendingCount}
        startDate={startDate}
        onStartDateChange={setStartDate}
        endDate={endDate}
        onEndDateChange={setEndDate}
        decisionMaker={decisionMaker}
        onDecisionMakerChange={setDecisionMaker}
        decisionMakers={decisionMakers}
        onReset={() => {
          setQ("")
          setFilter("all")
          setStartDate("")
          setEndDate("")
          setDecisionMaker(null)
        }}
        onRefresh={refresh}
        refreshing={loading}
      />

      <ReviewTableFrame>
        <Table className="table-fixed">
          <TableHeader>
            <TableRow>
              <TableHead className="w-[28%]">{t("review.subject")}</TableHead>
              <TableHead className="w-36">{t("review.handlingMethod")}</TableHead>
              <TableHead>{t("review.issueRationale")}</TableHead>
              <TableHead className="w-40">{t("review.provenance")}</TableHead>
              <TableHead className="w-24" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={5} className="h-20 text-center text-muted-foreground">{t("common.loading")}</TableCell></TableRow>
            ) : shown.length === 0 ? (
              <TableRow><TableCell colSpan={5} className="h-20 text-center text-muted-foreground">{t("common.noData")}</TableCell></TableRow>
            ) : shown.map((r) => (
              <TableRow key={r.key} className={r.pending ? (r.severity === "error" ? "bg-destructive/5" : "bg-amber-500/5") : undefined}>
                <TableCell className="max-w-[20rem]">
                  <div className="whitespace-normal break-words font-medium">{r.subject}</div>
                  <div className="mt-1 flex flex-wrap items-center gap-1.5">
                    <Badge variant="outline" className="text-[10px]">{r.type}</Badge>
                    {r.pending && r.conflict && (
                      <span className="text-[11px] text-muted-foreground">
                        {t("review.affectedEntities", { count: r.conflict.payload.entities.length })}
                      </span>
                    )}
                  </div>
                </TableCell>
                <TableCell>
                  <ReviewStatusBadge
                    tone={r.pending ? (r.severity === "error" ? "error" : "pending") : "neutral"}
                    title={r.pending ? (r.severity === "error" ? t("common.error") : t("common.warning")) : undefined}
                  >
                    {r.handling}
                  </ReviewStatusBadge>
                </TableCell>
                <TableCell className="text-xs">
                  {r.pending ? (
                    <p className="whitespace-normal break-words leading-relaxed text-muted-foreground">{r.detail}</p>
                  ) : r.onSaveReason ? (
                    <ReasonCell value={r.reason} canWrite={canWrite} onSave={r.onSaveReason} />
                  ) : r.reason ? (
                    <span className="flex items-center gap-1 text-primary">
                      <Sparkles className="h-3 w-3 shrink-0" /><span className="whitespace-normal leading-relaxed">{r.reason}</span>
                    </span>
                  ) : r.detail ? (
                    <span className="whitespace-normal leading-relaxed text-muted-foreground">{r.detail}</span>
                  ) : (
                    <span className="text-muted-foreground">—</span>
                  )}
                </TableCell>
                <TableCell><ReviewProvenance by={r.by} when={r.when} /></TableCell>
                <TableCell className="text-right">
                  {r.pending && r.conflict ? (
                    <ReviewActionButton onClick={() => setSelectedConflict(r.conflict ?? null)} />
                  ) : canWrite && r.onForget ? (
                    <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      title={t("review.forgetDecision")} disabled={busy !== null} onClick={r.onForget}>
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  ) : canWrite && r.onReopen ? (
                    <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-foreground"
                      title={t("review.reconsider")} disabled={busy !== null} onClick={r.onReopen}>
                      <RotateCcw className="h-3.5 w-3.5" />
                    </Button>
                  ) : null}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </ReviewTableFrame>

      <ReviewPagination page={p} total={filtered.length} onPageChange={setPage} />

      <ConflictReviewSheet
        ksId={ksId}
        conflict={selectedConflict}
        canWrite={canWrite}
        busy={selectedConflict ? busy === `c${selectedConflict.id}` : false}
        onClose={() => setSelectedConflict(null)}
        onResolve={(resolutionId) => { if (selectedConflict) resolve(selectedConflict, resolutionId) }}
        onDismiss={() => { if (selectedConflict) dismiss(selectedConflict) }}
      />
    </div>
  )
}
