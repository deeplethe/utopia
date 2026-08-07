import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import {
  ChevronDown, ChevronLeft, ChevronRight, RotateCcw, Search, Sparkles, Trash2, X,
} from "lucide-react"
import { api } from "@/lib/api"
import type { Conflict, Reconciliation } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { By, fmtWhen, HoverText, ReasonCell } from "@/components/review-bits"

const PAGE_SIZE = 20
type Filter = "all" | "pending" | "decided"

type Row = {
  key: string
  kind: "open" | "reconcile" | "duplicate"
  pending: boolean
  type: string
  subject: string
  detail: string | null
  status: string
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
  const [recs, setRecs] = useState<Reconciliation[]>([])
  const [dups, setDups] = useState<Conflict[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<string | null>(null)
  const [q, setQ] = useState("")
  const [filter, setFilter] = useState<Filter>("all")
  const [page, setPage] = useState(0)
  useEffect(() => { setPage(0) }, [q, filter])

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
      toast.error(`Failed to load: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [ksId])
  useEffect(() => { load() }, [load])

  const refresh = () => { load(); onChanged() }

  const resolve = async (c: Conflict, resolutionId: string) => {
    setBusy(`c${c.id}`)
    try { await api.resolveConflict(ksId, c.id, resolutionId); toast.success("Conflict resolved"); refresh() }
    catch (e) { toast.error(`Failed to resolve: ${(e as Error).message.replace(/^\d+:\s*/, "")}`) }
    finally { setBusy(null) }
  }
  const dismiss = async (c: Conflict) => {
    setBusy(`c${c.id}`)
    try { await api.dismissConflict(ksId, c.id); toast.success("Dismissed"); refresh() }
    catch (e) { toast.error(`Failed to dismiss: ${(e as Error).message.replace(/^\d+:\s*/, "")}`) }
    finally { setBusy(null) }
  }
  const revokeRec = async (r: Reconciliation) => {
    if (!confirm(`Forget the ${r.slot} reconciliation for "${r.property_label}"? The agent will re-decide it next time.`)) return
    setBusy(`r${r.id}`)
    try { await api.revokeReconciliation(ksId, r.id); toast.success("Forgotten"); refresh() }
    catch (e) { toast.error(`Failed: ${(e as Error).message.replace(/^\d+:\s*/, "")}`) }
    finally { setBusy(null) }
  }
  const reopenDup = async (c: Conflict) => {
    if (!confirm("Reconsider this pair? It returns to the pending queue for the judge to re-evaluate.")) return
    setBusy(`d${c.id}`)
    try { await api.reopenConflict(ksId, c.id); toast.success("Reopened"); refresh() }
    catch (e) { toast.error(`Failed: ${(e as Error).message.replace(/^\d+:\s*/, "")}`) }
    finally { setBusy(null) }
  }

  const rows: Row[] = [
    ...conflicts.map<Row>((c) => ({
      key: `c${c.id}`, kind: "open", pending: true, type: c.ctype, subject: c.title, detail: c.detail,
      status: "open", reason: c.payload.recommendation?.reason ?? null, by: null, when: c.created_at, conflict: c,
    })),
    ...recs.map<Row>((r) => ({
      key: `r${r.id}`, kind: "reconcile", pending: false, type: "Domain · range",
      subject: `${r.property_label} · ${r.slot}`, detail: null,
      status: r.choice + (r.chosen_label && r.choice !== "union" ? ` → ${r.chosen_label}` : ""),
      reason: r.reason, by: r.resolved_by, when: r.created_at,
      onForget: () => revokeRec(r),
      onSaveReason: async (v: string) => { await api.editReconciliationReason(ksId, r.id, v); refresh() },
    })),
    ...dups.map<Row>((c) => ({
      key: `d${c.id}`, kind: "duplicate", pending: false, type: "Duplicate class",
      subject: c.payload.entities.map((e) => e.label).join("  ·  "), detail: null,
      status: c.status === "dismissed" ? "kept distinct" : "merged", reason: null,
      by: null, when: c.resolved_at ?? c.created_at, onReopen: () => reopenDup(c),
    })),
  ]

  const pendingCount = conflicts.length
  const term = q.trim().toLowerCase()
  const filtered = rows.filter((r) =>
    (filter === "all" || (filter === "pending" ? r.pending : !r.pending)) &&
    `${r.type} ${r.subject} ${r.status} ${r.detail ?? ""}`.toLowerCase().includes(term))
  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE))
  const p = Math.min(page, pageCount - 1)
  const shown = filtered.slice(p * PAGE_SIZE, p * PAGE_SIZE + PAGE_SIZE)

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-semibold">Conflicts</h2>
          <p className="text-xs text-muted-foreground">
            Open conflicts and past decisions in one list — resolving one teaches the reconcile agent for next time.
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
              <TableHead className="w-32">Type</TableHead><TableHead>Subject</TableHead>
              <TableHead className="w-32">Status</TableHead><TableHead>Reason</TableHead>
              <TableHead className="w-24">By</TableHead><TableHead className="w-20">When</TableHead>
              <TableHead className="w-28" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={7} className="h-16 text-center text-muted-foreground">Loading…</TableCell></TableRow>
            ) : shown.length === 0 ? (
              <TableRow><TableCell colSpan={7} className="h-16 text-center text-muted-foreground">No data</TableCell></TableRow>
            ) : shown.map((r) => (
              <TableRow key={r.key} className={r.pending ? "bg-amber-500/5" : undefined}>
                <TableCell><Badge variant="outline" className="text-[10px]">{r.type}</Badge></TableCell>
                <TableCell className="max-w-[18rem] truncate font-medium" title={r.detail ?? r.subject}>{r.subject}</TableCell>
                <TableCell>
                  <Badge variant="outline" className={`text-[10px] ${r.pending ? "text-amber-600 dark:text-amber-400" : ""}`}>
                    {r.status}
                  </Badge>
                </TableCell>
                <TableCell className="text-xs">
                  {r.onSaveReason ? (
                    <ReasonCell value={r.reason} canWrite={canWrite} onSave={r.onSaveReason} />
                  ) : r.reason ? (
                    <span className="flex items-center gap-1 text-primary">
                      <Sparkles className="h-3 w-3 shrink-0" /><HoverText text={r.reason} className="text-primary" />
                    </span>
                  ) : r.detail ? (
                    <HoverText text={r.detail} className="text-muted-foreground" />
                  ) : (
                    <span className="text-muted-foreground">—</span>
                  )}
                </TableCell>
                <TableCell><By by={r.by} /></TableCell>
                <TableCell className="text-xs text-muted-foreground" title={r.when ?? ""}>{fmtWhen(r.when)}</TableCell>
                <TableCell className="text-right">
                  {!canWrite ? null : r.pending && r.conflict ? (
                    <ResolveMenu c={r.conflict} busy={busy === `c${r.conflict.id}`} onResolve={resolve} onDismiss={dismiss} />
                  ) : r.onForget ? (
                    <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      title="Forget this decision" disabled={busy !== null} onClick={r.onForget}>
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  ) : r.onReopen ? (
                    <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-foreground"
                      title="Reconsider (reopen)" disabled={busy !== null} onClick={r.onReopen}>
                      <RotateCcw className="h-3.5 w-3.5" />
                    </Button>
                  ) : null}
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

/** Compact per-conflict action: its resolutions (the recommended one starred and first) + Dismiss. */
function ResolveMenu({
  c, busy, onResolve, onDismiss,
}: {
  c: Conflict
  busy: boolean
  onResolve: (c: Conflict, resolutionId: string) => void
  onDismiss: (c: Conflict) => void
}) {
  const recId = c.payload.recommendation?.resolution_id
  const resolutions = [...c.payload.resolutions].sort(
    (a, b) => Number(b.id === recId) - Number(a.id === recId),
  )
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="sm" variant="outline" className="h-7 gap-1" disabled={busy}>
          Resolve <ChevronDown className="h-3.5 w-3.5" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="max-w-[18rem]">
        {resolutions.map((r) => (
          <DropdownMenuItem key={r.id} onClick={() => onResolve(c, r.id)}>
            {r.id === recId && <Sparkles className="h-3.5 w-3.5 text-primary" />}
            {r.label}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={() => onDismiss(c)} className="text-muted-foreground">
          <X className="h-3.5 w-3.5" /> Dismiss
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
