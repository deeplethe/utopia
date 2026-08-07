import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { Check, ChevronDown, ChevronLeft, ChevronRight, Search, Sparkles, Trash2 } from "lucide-react"
import { api } from "@/lib/api"
import type { ResolutionDecision, ResolutionQueueItem } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { By, fmtWhen, ReasonCell } from "@/components/review-bits"

const PAGE_SIZE = 20
type Filter = "all" | "pending" | "decided"

type Row = {
  id: number; surface: string; classLabel: string | null; status: string; pending: boolean
  candidates: { iri: string; label: string; score: number }[]
  individualLabel: string | null; confidence: number | null; reason: string | null
  by: string | null; when: string | null
}

/**
 * Entity resolution — one table for the whole queue: pending mentions on top, then the resolved
 * decisions, filterable by status and searchable. A pending row acts via a compact "Resolve" menu;
 * a decided row can be forgotten (the agent re-judges that surface next time).
 */
export default function ResolutionPanel({
  ksId, canWrite, onChanged,
}: {
  ksId: number
  canWrite: boolean
  onChanged?: () => void
}) {
  const [rows, setRows] = useState<Row[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<number | null>(null)
  const [q, setQ] = useState("")
  const [filter, setFilter] = useState<Filter>("all")
  const [page, setPage] = useState(0)
  useEffect(() => { setPage(0) }, [q, filter])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [queue, decisions] = await Promise.all([
        api.getResolutionQueue(ksId, { limit: 500 }),
        api.getResolutionDecisions(ksId, { limit: 500 }),
      ])
      const pending: Row[] = queue.items.map((i: ResolutionQueueItem) => ({
        id: i.id, surface: i.surface_form, classLabel: i.class_label, status: "pending", pending: true,
        candidates: i.candidates, individualLabel: null, confidence: i.confidence, reason: null, by: null, when: null,
      }))
      const decided: Row[] = decisions.items.map((d: ResolutionDecision) => ({
        id: d.id, surface: d.surface_form, classLabel: d.class_label, status: d.status, pending: false,
        candidates: [], individualLabel: d.individual_label, confidence: d.confidence, reason: d.reason,
        by: d.resolved_by, when: d.created_at,
      }))
      setRows([...pending, ...decided]) // pending first
    } catch (e) {
      toast.error(`Failed to load: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [ksId])
  useEffect(() => { load() }, [load])

  const resolve = async (id: number, action: "match" | "new", iri?: string) => {
    setBusy(id)
    try {
      const res = await api.resolveQueueItem(ksId, id, action, iri)
      toast.success(res.summary)
      load(); onChanged?.()
    } catch (e) {
      toast.error(`Resolve failed: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    } finally { setBusy(null) }
  }
  const forget = async (r: Row) => {
    if (!confirm(`Forget the resolution memory for "${r.surface}"? The agent will re-judge it next time.`)) return
    setBusy(r.id)
    try { await api.revokeResolutionDecision(ksId, r.id); toast.success("Forgotten"); load(); onChanged?.() }
    catch (e) { toast.error(`Failed: ${(e as Error).message.replace(/^\d+:\s*/, "")}`) }
    finally { setBusy(null) }
  }

  const pendingCount = rows.filter((r) => r.pending).length
  const term = q.trim().toLowerCase()
  const filtered = rows.filter((r) =>
    (filter === "all" || (filter === "pending" ? r.pending : !r.pending)) &&
    `${r.surface} ${r.classLabel ?? ""}`.toLowerCase().includes(term))
  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE))
  const p = Math.min(page, pageCount - 1)
  const shown = filtered.slice(p * PAGE_SIZE, p * PAGE_SIZE + PAGE_SIZE)

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-semibold">Entity resolution</h2>
          <p className="text-xs text-muted-foreground">
            Pending mentions and past decisions in one list — resolving one teaches the extractor for next time.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search…" className="h-8 w-44 pl-7 text-sm" />
          </div>
          <Select value={filter} onValueChange={(v) => setFilter(v as Filter)}>
            <SelectTrigger className="h-8 w-32 text-sm"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All</SelectItem>
              <SelectItem value="pending">Pending{pendingCount ? ` (${pendingCount})` : ""}</SelectItem>
              <SelectItem value="decided">Decided</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Mention</TableHead><TableHead>Class</TableHead><TableHead className="w-24">Status</TableHead>
              <TableHead>Individual</TableHead><TableHead>Reason</TableHead><TableHead className="w-24">By</TableHead>
              <TableHead className="w-20">When</TableHead><TableHead className="w-28" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={8} className="h-16 text-center text-muted-foreground">Loading…</TableCell></TableRow>
            ) : shown.length === 0 ? (
              <TableRow><TableCell colSpan={8} className="h-16 text-center text-muted-foreground">No data</TableCell></TableRow>
            ) : shown.map((r) => (
              <TableRow key={r.id} className={r.pending ? "bg-amber-500/5" : undefined}>
                <TableCell className="font-medium">{r.surface}</TableCell>
                <TableCell className="text-muted-foreground">{r.classLabel ?? "—"}</TableCell>
                <TableCell>
                  <Badge variant="outline" className={`text-[10px] ${r.pending ? "text-amber-600 dark:text-amber-400" : ""}`}>
                    {r.status}{!r.pending && r.confidence != null ? ` · ${r.confidence.toFixed(2)}` : ""}
                  </Badge>
                </TableCell>
                <TableCell className="text-muted-foreground">{r.individualLabel ?? "—"}</TableCell>
                <TableCell className="text-xs">
                  {r.pending ? (
                    <span className="text-muted-foreground">—</span>
                  ) : (
                    <ReasonCell value={r.reason} canWrite={canWrite}
                      onSave={async (v) => { await api.editResolutionReason(ksId, r.id, v); load(); onChanged?.() }} />
                  )}
                </TableCell>
                <TableCell><By by={r.by} /></TableCell>
                <TableCell className="text-xs text-muted-foreground" title={r.when ?? ""}>{fmtWhen(r.when)}</TableCell>
                <TableCell className="text-right">
                  {!canWrite ? null : r.pending ? (
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button size="sm" variant="outline" className="h-7 gap-1" disabled={busy === r.id}>
                          Resolve <ChevronDown className="h-3.5 w-3.5" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        {r.candidates.map((c) => (
                          <DropdownMenuItem key={c.iri} onClick={() => resolve(r.id, "match", c.iri)}>
                            <Check className="h-3.5 w-3.5" /> Same as “{c.label}” · {c.score}
                          </DropdownMenuItem>
                        ))}
                        <DropdownMenuItem onClick={() => resolve(r.id, "new")}>
                          <Sparkles className="h-3.5 w-3.5" /> New individual
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  ) : (
                    <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      title="Forget this decision" disabled={busy !== null} onClick={() => forget(r)}>
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      {filtered.length > PAGE_SIZE && (
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>{p * PAGE_SIZE + 1}–{Math.min(filtered.length, (p + 1) * PAGE_SIZE)} of {filtered.length}</span>
          <div className="flex gap-1">
            <Button size="sm" variant="outline" className="h-7 w-7 p-0" disabled={p === 0} onClick={() => setPage(p - 1)}>
              <ChevronLeft className="h-4 w-4" />
            </Button>
            <Button size="sm" variant="outline" className="h-7 w-7 p-0" disabled={p >= pageCount - 1} onClick={() => setPage(p + 1)}>
              <ChevronRight className="h-4 w-4" />
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}
