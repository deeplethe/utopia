import { useCallback, useEffect, useMemo, useState } from "react"
import { CheckCircle2, Loader2, RefreshCw, XCircle } from "lucide-react"
import { toast } from "sonner"

import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import type { ExtractionJob } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { ReviewPagination } from "@/components/review-bits"

const PAGE_SIZE = 5
type StatusFilter = "all" | ExtractionJob["status"]

function formatTimestamp(value: string | null, locale: string, fallback: string) {
  if (!value) return fallback
  return new Date(value).toLocaleString(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  })
}

function JobStatus({ status }: { status: ExtractionJob["status"] }) {
  const { t } = useI18n()
  const config = {
    pending: { className: "text-amber-600 dark:text-amber-400", icon: Loader2, spin: false },
    running: { className: "text-blue-600 dark:text-blue-400", icon: Loader2, spin: true },
    completed: { className: "text-emerald-600 dark:text-emerald-400", icon: CheckCircle2, spin: false },
    failed: { className: "text-red-600 dark:text-red-400", icon: XCircle, spin: false },
  }[status]
  const Icon = config.icon
  return (
    <span className={`inline-flex items-center gap-1.5 text-xs font-medium ${config.className}`}>
      <Icon className={`h-3.5 w-3.5 ${config.spin ? "animate-spin" : ""}`} />
      {t(`extractionQueue.status.${status}`)}
    </span>
  )
}

function JobResult({ job }: { job: ExtractionJob }) {
  const { t } = useI18n()
  const rows: string[] = []
  if (job.kind !== "abox") {
    rows.push(t("extractionQueue.schemaResult", {
      classes: job.classes_added,
      properties: job.properties_added,
      axioms: job.axioms_added,
    }))
  }
  if (job.kind !== "tbox") {
    rows.push(t("extractionQueue.instanceResult", {
      individuals: job.individuals_added,
      assertions: job.assertions_added,
      pending: job.pending_added,
    }))
  }
  if (job.terms_added || job.terminology_proposals) {
    rows.push(t("extractionQueue.termResult", {
      terms: job.terms_added,
      proposals: job.terminology_proposals,
    }))
  }
  return (
    <div className="space-y-0.5 text-xs text-muted-foreground">
      {rows.map((row) => <div key={row}>{row}</div>)}
    </div>
  )
}

export default function ExtractionQueuePanel({ ksId, showTitle = true }: { ksId: number; showTitle?: boolean }) {
  const { locale, t } = useI18n()
  const [jobs, setJobs] = useState<ExtractionJob[]>([])
  const [filter, setFilter] = useState<StatusFilter>("all")
  const [page, setPage] = useState(0)
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)

  const load = useCallback(async (silent = false) => {
    if (silent) setRefreshing(true)
    else setLoading(true)
    try {
      setJobs(await api.listJobs(ksId))
    } catch (error) {
      if (!silent) toast.error(t("extractionQueue.loadFailed", { error: (error as Error).message }))
    } finally {
      setLoading(false)
      setRefreshing(false)
    }
  }, [ksId, t])

  useEffect(() => { void load() }, [load])
  const hasActiveJobs = jobs.some((job) => job.status === "pending" || job.status === "running")
  useEffect(() => {
    if (!hasActiveJobs) return
    const timer = window.setInterval(() => { void load(true) }, 2000)
    return () => window.clearInterval(timer)
  }, [hasActiveJobs, load])

  const filtered = useMemo(
    () => filter === "all" ? jobs : jobs.filter((job) => job.status === filter),
    [filter, jobs],
  )
  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE))
  useEffect(() => { setPage(0) }, [filter])
  useEffect(() => { setPage((current) => Math.min(current, pageCount - 1)) }, [pageCount])
  const shown = filtered.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE)

  return (
    <div className="min-w-0 space-y-3">
      <div className={`flex flex-wrap items-center gap-3 ${showTitle ? "justify-between" : "justify-end"}`}>
        {showTitle && <h2 className="text-sm font-semibold">{t("extractionQueue.title")}</h2>}
        <div className="flex items-center gap-2">
          <Select value={filter} onValueChange={(value) => setFilter(value as StatusFilter)}>
            <SelectTrigger className="h-8 w-32 text-sm"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t("common.all")}</SelectItem>
              <SelectItem value="pending">{t("extractionQueue.status.pending")}</SelectItem>
              <SelectItem value="running">{t("extractionQueue.status.running")}</SelectItem>
              <SelectItem value="completed">{t("extractionQueue.status.completed")}</SelectItem>
              <SelectItem value="failed">{t("extractionQueue.status.failed")}</SelectItem>
            </SelectContent>
          </Select>
          <Button size="icon" variant="outline" className="h-8 w-8" onClick={() => load(true)} disabled={refreshing} title={t("common.refresh")}>
            <RefreshCw className={`h-3.5 w-3.5 ${refreshing ? "animate-spin" : ""}`} />
          </Button>
        </div>
      </div>

      <div className="overflow-x-auto rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="min-w-44">{t("extractionQueue.job")}</TableHead>
              <TableHead className="w-28">{t("common.status")}</TableHead>
              <TableHead>{t("extractionQueue.phase")}</TableHead>
              <TableHead className="min-w-44">{t("extractionQueue.progress")}</TableHead>
              <TableHead className="min-w-52">{t("extractionQueue.result")}</TableHead>
              <TableHead className="min-w-44">{t("extractionQueue.time")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={6} className="h-24 text-center text-muted-foreground">
                <Loader2 className="mr-2 inline h-4 w-4 animate-spin" />{t("common.loading")}
              </TableCell></TableRow>
            ) : shown.length === 0 ? (
              <TableRow><TableCell colSpan={6} className="h-24 text-center text-muted-foreground">{t("extractionQueue.empty")}</TableCell></TableRow>
            ) : shown.map((job) => {
              const progress = job.total_chunks > 0
                ? Math.min(100, Math.round(job.processed_chunks / job.total_chunks * 100))
                : 0
              const phase = job.phase ? t(`extract.phase.${job.phase}`) : t(`extractionQueue.status.${job.status}`)
              return (
                <TableRow key={job.id}>
                  <TableCell>
                    <div className="font-medium">#{job.id}</div>
                    <div className="text-[11px] text-muted-foreground">{t(`extractionQueue.kind.${job.kind}`)}</div>
                    <div className="max-w-40 truncate text-[11px] text-muted-foreground" title={job.model}>{job.model || "—"}</div>
                  </TableCell>
                  <TableCell><JobStatus status={job.status} /></TableCell>
                  <TableCell className="max-w-64 text-xs">
                    <div>{phase}</div>
                    {job.error && <div className="mt-1 line-clamp-2 text-red-600 dark:text-red-400" title={job.error}>{job.error}</div>}
                  </TableCell>
                  <TableCell>
                    <div className="flex items-center justify-between gap-3 text-xs tabular-nums">
                      <span>{job.processed_chunks}/{job.total_chunks}</span>
                      <span className="text-muted-foreground">{progress}%</span>
                    </div>
                    <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-muted">
                      <div className="h-full bg-foreground/70 transition-all" style={{ width: `${progress}%` }} />
                    </div>
                  </TableCell>
                  <TableCell><JobResult job={job} /></TableCell>
                  <TableCell className="whitespace-nowrap text-xs text-muted-foreground">
                    <div>{formatTimestamp(job.created_at, locale, "—")}</div>
                    <div>{job.finished_at ? `→ ${formatTimestamp(job.finished_at, locale, "—")}` : "—"}</div>
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      </div>

      <ReviewPagination page={page} total={filtered.length} onPageChange={setPage} />
    </div>
  )
}
