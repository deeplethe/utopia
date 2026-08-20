import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { Trash2 } from "lucide-react"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type { ResolutionDecision, ResolutionQueueItem } from "@/lib/types"
import ResolutionReviewSheet from "@/components/ResolutionReviewSheet"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import {
  REVIEW_PAGE_SIZE, ReasonCell, ReviewActionButton, ReviewPagination, ReviewProvenance,
  ReviewQueueHeader, ReviewStatusBadge, ReviewTableFrame, type ReviewFilter, type ReviewStatusTone,
  matchesReviewFilters,
} from "@/components/review-bits"

type Row = {
  id: number
  surface: string
  classLabel: string | null
  decisionStatus: ResolutionDecision["status"] | null
  status: string
  statusTone: ReviewStatusTone
  pending: boolean
  candidates: { iri: string; label: string; score: number }[]
  individualLabel: string | null
  individualDeleted: boolean
  reason: string | null
  reasonFallback: string | null
  by: string | null
  when: string | null
  queueItem?: ResolutionQueueItem
}

export default function ResolutionPanel({
  ksId, canWrite, onChanged,
}: {
  ksId: number
  canWrite: boolean
  onChanged?: () => void
}) {
  const { t } = useI18n()
  const confirmAction = useConfirm()
  const [rows, setRows] = useState<Row[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<number | null>(null)
  const [selected, setSelected] = useState<ResolutionQueueItem | null>(null)
  const [query, setQuery] = useState("")
  const [filter, setFilter] = useState<ReviewFilter>("all")
  const [startDate, setStartDate] = useState("")
  const [endDate, setEndDate] = useState("")
  const [decisionMaker, setDecisionMaker] = useState<string | null>(null)
  const [page, setPage] = useState(0)
  useEffect(() => { setPage(0) }, [query, filter, startDate, endDate, decisionMaker])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [queue, decisions] = await Promise.all([
        api.getResolutionQueue(ksId, { limit: 500 }),
        api.getResolutionDecisions(ksId, { limit: 500 }),
      ])
      const pending: Row[] = queue.items.map((item: ResolutionQueueItem) => ({
        id: item.id,
        surface: item.surface_form,
        classLabel: item.class_label,
        decisionStatus: null,
        status: t("review.status.pending"),
        statusTone: "pending",
        pending: true,
        candidates: item.candidates,
        individualLabel: null,
        individualDeleted: false,
        reason: null,
        reasonFallback: null,
        by: null,
        when: item.created_at,
        queueItem: item,
      }))
      const decided: Row[] = decisions.items.map((decision: ResolutionDecision) => ({
        id: decision.id,
        surface: decision.surface_form,
        classLabel: decision.class_label,
        decisionStatus: decision.status,
        status: decision.status === "matched" ? t("review.status.matched")
          : decision.status === "new" ? t("review.status.new")
            : decision.status === "rejected" ? t("review.status.rejected")
              : decision.status === "deferred" ? t("review.status.deferred")
                : t("review.status.distinct"),
        statusTone: decision.status === "deferred" ? "pending"
          : decision.status === "rejected" || decision.status === "distinct" ? "neutral" : "success",
        pending: false,
        candidates: [],
        individualLabel: decision.individual_label,
        individualDeleted: decision.individual_deleted,
        reason: decision.reason,
        reasonFallback: `${decision.status === "new" ? t("review.resolution.reason.new")
          : decision.status === "matched" ? t("review.resolution.reason.matched")
            : t("review.resolution.reason.distinct")}${decision.individual_deleted
          ? ` ${t("review.resolution.targetDeleted")}` : ""}`,
        by: decision.resolved_by,
        when: decision.resolved_at ?? decision.created_at,
      }))
      setRows([...pending, ...decided])
    } catch (error) {
      toast.error(t("common.failedLoad", { error: (error as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [ksId, t])
  useEffect(() => { load() }, [load])

  const resolve = async (
    item: ResolutionQueueItem,
    action: "match" | "new" | "reject" | "defer",
    options: { iri?: string; reason?: string; reviewAfter?: string } = {},
  ) => {
    const id = item.id
    setBusy(id)
    try {
      const result = await api.resolveQueueItem(ksId, id, {
        action,
        individual_iri: options.iri,
        reason: options.reason,
        review_after: options.reviewAfter || undefined,
        expected_updated_at: item.updated_at,
      })
      toast.success(result.summary)
      setSelected(null)
      await load()
      onChanged?.()
    } catch (error) {
      toast.error(t("review.resolveFailed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setBusy(null)
    }
  }

  const merge = async (sourceIri: string, canonicalIri: string, reason: string) => {
    if (!selected) return
    setBusy(selected.id)
    try {
      await api.mergeIndividuals(
        ksId, sourceIri, canonicalIri, reason, selected.id, selected.updated_at,
      )
      toast.success(t("review.resolution.mergeComplete"))
      setSelected(null)
      await load()
      onChanged?.()
    } catch (error) {
      toast.error(t("review.resolveFailed", { error: (error as Error).message }))
    } finally {
      setBusy(null)
    }
  }

  const forget = async (row: Row) => {
    if (!await confirmAction(t("review.forgetResolution", { name: row.surface }), { destructive: true })) return
    setBusy(row.id)
    try {
      await api.revokeResolutionDecision(ksId, row.id)
      toast.success(t("review.forgotten"))
      await load()
      onChanged?.()
    } catch (error) {
      toast.error(t("review.failed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setBusy(null)
    }
  }

  const pendingCount = rows.filter((row) => row.pending).length
  const decisionMakers = rows.flatMap((row) => row.by ? [row.by] : [])
  const term = query.trim().toLowerCase()
  const filtered = rows.filter((row) =>
    (filter === "all" || (filter === "pending" ? row.pending : !row.pending))
    && `${row.surface} ${row.classLabel ?? ""} ${row.individualLabel ?? ""} ${row.reason ?? ""} ${row.reasonFallback ?? ""}`.toLowerCase().includes(term)
    && matchesReviewFilters({ when: row.when, by: row.by, startDate, endDate, decisionMaker }))
  const pageCount = Math.max(1, Math.ceil(filtered.length / REVIEW_PAGE_SIZE))
  const safePage = Math.min(page, pageCount - 1)
  const shown = filtered.slice(safePage * REVIEW_PAGE_SIZE, (safePage + 1) * REVIEW_PAGE_SIZE)

  return (
    <div className="space-y-4">
      <ReviewQueueHeader
        title={t("review.entityResolution.title")}
        query={query}
        onQueryChange={setQuery}
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
          setQuery("")
          setFilter("all")
          setStartDate("")
          setEndDate("")
          setDecisionMaker(null)
        }}
        onRefresh={load}
        refreshing={loading}
      />

      <ReviewTableFrame>
        <Table className="table-fixed">
          <TableHeader>
            <TableRow>
              <TableHead className="w-[28%]">{t("review.subject")}</TableHead>
              <TableHead className="w-36">{t("common.status")}</TableHead>
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
            ) : shown.map((row) => (
              <TableRow key={`${row.pending ? "pending" : "decided"}-${row.id}`} className={row.pending ? "bg-amber-500/5" : undefined}>
                <TableCell className="max-w-[20rem]">
                  <div className="whitespace-normal break-words font-medium">{row.surface}</div>
                  {row.classLabel && <Badge variant="outline" className="mt-1 text-[10px]">{row.classLabel}</Badge>}
                  {!row.pending && row.individualLabel
                    && (row.decisionStatus === "matched" || row.individualDeleted) && (
                    <div className="mt-1 flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
                      <span
                        className={`break-all ${row.individualDeleted ? "line-through decoration-1" : ""}`}
                        title={row.individualDeleted ? t("common.deleted") : undefined}
                      >
                        {t("review.resolution.target", { name: row.individualLabel })}
                      </span>
                    </div>
                  )}
                </TableCell>
                <TableCell>
                  <ReviewStatusBadge tone={row.statusTone}>
                    {row.status}
                  </ReviewStatusBadge>
                </TableCell>
                <TableCell className="text-xs">
                  {row.pending ? (
                    <p className="text-muted-foreground">{t("review.resolution.candidates", { count: row.candidates.length })}</p>
                  ) : (
                    <ReasonCell
                      value={row.reason}
                      fallback={row.reasonFallback ?? undefined}
                      canWrite={canWrite}
                      onSave={async (value) => {
                        await api.editResolutionReason(ksId, row.id, value)
                        await load()
                        onChanged?.()
                      }}
                    />
                  )}
                </TableCell>
                <TableCell><ReviewProvenance by={row.by} when={row.when} /></TableCell>
                <TableCell className="text-right">
                  {row.pending && row.queueItem ? (
                    <ReviewActionButton onClick={() => setSelected(row.queueItem ?? null)} disabled={busy === row.id} />
                  ) : !canWrite ? null : (
                    <Button
                      size="icon"
                      variant="ghost"
                      className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      title={t("review.forgetDecision")}
                      disabled={busy !== null}
                      onClick={() => forget(row)}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </ReviewTableFrame>

      <ReviewPagination page={safePage} total={filtered.length} onPageChange={setPage} />

      <ResolutionReviewSheet
        ksId={ksId}
        item={selected}
        canWrite={canWrite}
        busy={selected ? busy === selected.id : false}
        onClose={() => setSelected(null)}
        onResolve={(action, options) => { if (selected) resolve(selected, action, options) }}
        onMerge={merge}
      />
    </div>
  )
}
