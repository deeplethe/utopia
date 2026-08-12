import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { Trash2 } from "lucide-react"
import { api } from "@/lib/api"
import { useI18n, type MessageKey } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type { ValidationDecision, ValidationFix, ValidationResult, Violation } from "@/lib/types"
import ValidationReviewSheet from "@/components/ValidationReviewSheet"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import {
  REVIEW_PAGE_SIZE, ReviewActionButton, ReviewPagination, ReviewProvenance, ReviewQueueHeader,
  ReviewStatusBadge, ReviewTableFrame, type ReviewFilter, type ReviewStatusTone,
  matchesReviewFilters,
} from "@/components/review-bits"

const TYPE_KEY: Record<Violation["type"], MessageKey> = {
  placeholder: "review.validation.placeholder",
  type_count: "review.validation.typeCount",
  role: "review.validation.roleConflict",
  disjoint: "review.validation.disjointTypes",
  domain: "review.validation.domain",
  range: "review.validation.range",
  datatype: "review.validation.datatype",
}

type Row = {
  key: string
  pending: boolean
  type: string
  subject: string
  status: string
  statusTone: ReviewStatusTone
  reason: string | null
  by: string | null
  when: string | null
  violation?: Violation
  onForget?: () => void
}

export default function ValidationPanel({
  ksId, canWrite, onChanged,
}: {
  ksId: number
  canWrite: boolean
  onChanged?: () => void
}) {
  const { t } = useI18n()
  const confirmAction = useConfirm()
  const [result, setResult] = useState<ValidationResult | null>(null)
  const [decisions, setDecisions] = useState<ValidationDecision[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<string | null>(null)
  const [selected, setSelected] = useState<Violation | null>(null)
  const [query, setQuery] = useState("")
  const [filter, setFilter] = useState<ReviewFilter>("all")
  const [startDate, setStartDate] = useState("")
  const [endDate, setEndDate] = useState("")
  const [decisionMaker, setDecisionMaker] = useState<string | null>(null)
  const [page, setPage] = useState(0)
  useEffect(() => { setPage(0) }, [query, filter, startDate, endDate, decisionMaker])

  const loadDecisions = useCallback(async () => {
    try {
      const response = await api.listValidationDecisions(ksId, { limit: 500 })
      setDecisions(response.items)
    } catch (error) {
      toast.error(t("review.validation.loadDecisionsFailed", { error: (error as Error).message }))
    }
  }, [ksId, t])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [validation] = await Promise.all([api.validateAbox(ksId), loadDecisions()])
      setResult(validation)
    } catch (error) {
      toast.error(t("review.validation.failed", { error: (error as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [ksId, loadDecisions, t])
  useEffect(() => { load() }, [load])

  const applyFix = useCallback(async (violation: Violation, fix: ValidationFix) => {
    setBusy(violation.id + fix.id)
    try {
      const validation = await api.fixViolation(ksId, fix.op, `${fix.label} — ${violation.summary}`)
      setResult(validation)
      setSelected(null)
      await loadDecisions()
      toast.success(t("review.validation.appliedFix"))
      onChanged?.()
    } catch (error) {
      toast.error(t("review.validation.fixFailed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setBusy(null)
    }
  }, [ksId, loadDecisions, onChanged, t])

  const forget = useCallback(async (decision: ValidationDecision) => {
    if (!await confirmAction(t("review.validation.forgetRuleConfirm", { name: decision.property_label }), { destructive: true })) return
    setBusy(`d${decision.id}`)
    try {
      await api.revokeValidationDecision(ksId, decision.id)
      toast.success(t("review.forgotten"))
      await loadDecisions()
      onChanged?.()
    } catch (error) {
      toast.error(t("review.failed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setBusy(null)
    }
  }, [confirmAction, ksId, loadDecisions, onChanged, t])

  const counts = result?.counts
  const violations = result?.violations ?? []
  const rows: Row[] = [
    ...violations.map<Row>((violation) => ({
      key: `v${violation.id}`,
      pending: true,
      type: t(TYPE_KEY[violation.type]),
      subject: violation.individual.label,
      status: t("common.pending"),
      statusTone: violation.severity === "error" ? "error" : "warning",
      reason: violation.summary,
      by: null,
      when: null,
      violation,
    })),
    ...decisions.map<Row>((decision) => ({
      key: `d${decision.id}`,
      pending: false,
      type: t("review.validation.datatypeRule"),
      subject: decision.property_label,
      status: decision.action === "relax" ? t("review.validation.toText") : t("review.validation.removeNoise"),
      statusTone: "neutral",
      reason: decision.reason,
      by: decision.resolved_by,
      when: decision.created_at,
      onForget: () => forget(decision),
    })),
  ]

  const decisionMakers = rows.flatMap((row) => row.by ? [row.by] : [])
  const term = query.trim().toLowerCase()
  const filtered = rows.filter((row) =>
    (filter === "all" || (filter === "pending" ? row.pending : !row.pending))
    && `${row.type} ${row.subject} ${row.status} ${row.reason ?? ""}`.toLowerCase().includes(term)
    && matchesReviewFilters({ when: row.when, by: row.by, startDate, endDate, decisionMaker }))
  const pageCount = Math.max(1, Math.ceil(filtered.length / REVIEW_PAGE_SIZE))
  const safePage = Math.min(page, pageCount - 1)
  const shown = filtered.slice(safePage * REVIEW_PAGE_SIZE, (safePage + 1) * REVIEW_PAGE_SIZE)

  return (
    <div className="space-y-4">
      <ReviewQueueHeader
        title={t("review.validation.title")}
        query={query}
        onQueryChange={setQuery}
        filter={filter}
        onFilterChange={setFilter}
        pendingCount={violations.length}
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
        summary={counts && (counts.error > 0 || counts.warning > 0) ? (
          <div className="flex items-center gap-1.5">
            {counts.error > 0 && <ReviewStatusBadge tone="error">{t(counts.error === 1 ? "review.error" : "review.errors", { count: counts.error })}</ReviewStatusBadge>}
            {counts.warning > 0 && <ReviewStatusBadge tone="warning">{t(counts.warning === 1 ? "review.warning" : "review.warnings", { count: counts.warning })}</ReviewStatusBadge>}
          </div>
        ) : undefined}
      />

      {result?.truncated && (
        <p className="text-xs text-muted-foreground">{t("review.validation.truncated", { count: violations.length })}</p>
      )}

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
              <TableRow><TableCell colSpan={5} className="h-20 text-center text-muted-foreground">{t("review.validation.checking")}</TableCell></TableRow>
            ) : shown.length === 0 ? (
              <TableRow><TableCell colSpan={5} className="h-20 text-center text-muted-foreground">{t("common.noData")}</TableCell></TableRow>
            ) : shown.map((row) => (
              <TableRow key={row.key} className={row.pending ? (row.statusTone === "error" ? "bg-destructive/5" : "bg-amber-500/5") : undefined}>
                <TableCell className="max-w-[20rem]">
                  <div className="whitespace-normal break-words font-medium">{row.subject}</div>
                  <Badge variant="outline" className="mt-1 text-[10px]">{row.type}</Badge>
                </TableCell>
                <TableCell>
                  <ReviewStatusBadge
                    tone={row.statusTone}
                    title={row.pending ? (row.statusTone === "error" ? t("common.error") : t("common.warning")) : undefined}
                  >
                    {row.status}
                  </ReviewStatusBadge>
                </TableCell>
                <TableCell className="text-xs leading-relaxed text-muted-foreground">{row.reason || "—"}</TableCell>
                <TableCell><ReviewProvenance by={row.by} when={row.when} /></TableCell>
                <TableCell className="text-right">
                  {row.pending && row.violation ? (
                    <ReviewActionButton onClick={() => setSelected(row.violation ?? null)} disabled={busy !== null} />
                  ) : canWrite && row.onForget ? (
                    <Button
                      size="icon"
                      variant="ghost"
                      className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      title={t("review.validation.forgetRule")}
                      disabled={busy !== null}
                      onClick={row.onForget}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  ) : null}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </ReviewTableFrame>

      <ReviewPagination page={safePage} total={filtered.length} onPageChange={setPage} />

      <ValidationReviewSheet
        violation={selected}
        typeLabel={selected ? t(TYPE_KEY[selected.type]) : ""}
        canWrite={canWrite}
        busy={selected ? busy?.startsWith(selected.id) ?? false : false}
        onClose={() => setSelected(null)}
        onFix={(fix) => { if (selected) applyFix(selected, fix) }}
      />
    </div>
  )
}
