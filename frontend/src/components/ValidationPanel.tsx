import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import {
  ChevronDown, ChevronLeft, ChevronRight, Loader2, RefreshCw, Search, Trash2,
} from "lucide-react"
import { api } from "@/lib/api"
import type { ValidationDecision, ValidationFix, ValidationResult, Violation } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { By, fmtWhen } from "@/components/review-bits"

const TYPE_LABEL: Record<Violation["type"], string> = {
  disjoint: "Disjoint types", domain: "Domain", range: "Range", datatype: "Datatype",
}
const PAGE_SIZE = 20
type Filter = "all" | "pending" | "decided"

type Row = {
  key: string
  pending: boolean
  type: string
  subject: string
  status: string
  statusTone: "error" | "warning" | "neutral"
  reason: string | null
  by: string | null
  when: string | null
  violation?: Violation // pending rows carry the source violation for the fix menu
  onForget?: () => void
}

/**
 * Validation — one table: current ABox violations (pending, on top) alongside the datatype-fix
 * rules the validation agent has learned. A pending violation is fixed via a compact "Fix" menu on
 * the instance data; a learned rule can be forgotten so the agent re-judges that property next time.
 * Violations are recomputed fresh from the graph, so the pending rows always reflect the current state.
 */
export default function ValidationPanel({
  ksId, canWrite, onChanged,
}: {
  ksId: number
  canWrite: boolean
  onChanged?: () => void
}) {
  const [result, setResult] = useState<ValidationResult | null>(null)
  const [decisions, setDecisions] = useState<ValidationDecision[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<string | null>(null)
  const [q, setQ] = useState("")
  const [filter, setFilter] = useState<Filter>("all")
  const [page, setPage] = useState(0)
  useEffect(() => { setPage(0) }, [q, filter])

  const loadDecisions = useCallback(async () => {
    try {
      const res = await api.listValidationDecisions(ksId, { limit: 500 })
      setDecisions(res.items)
    } catch (e) {
      toast.error(`Failed to load decisions: ${(e as Error).message}`)
    }
  }, [ksId])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [v] = await Promise.all([api.validateAbox(ksId), loadDecisions()])
      setResult(v)
    } catch (e) {
      toast.error(`Validation failed: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [ksId, loadDecisions])
  useEffect(() => { load() }, [load])

  const applyFix = useCallback(async (v: Violation, fix: ValidationFix) => {
    setBusy(v.id + fix.id)
    try {
      const res = await api.fixViolation(ksId, fix.op, `${fix.label} — ${v.summary}`)
      setResult(res)
      await loadDecisions() // a relax fix records a new learned rule
      toast.success("Applied fix")
      onChanged?.()
    } catch (e) {
      toast.error(`Fix failed: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    } finally {
      setBusy(null)
    }
  }, [ksId, loadDecisions, onChanged])

  const forget = useCallback(async (d: ValidationDecision) => {
    if (!confirm(`Forget the fix rule for "${d.property_label}"? The agent will re-judge it next time. (The schema change already applied is not undone.)`)) return
    setBusy(`d${d.id}`)
    try {
      await api.revokeValidationDecision(ksId, d.id)
      toast.success("Forgotten")
      await loadDecisions()
      onChanged?.()
    } catch (e) {
      toast.error(`Failed: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    } finally {
      setBusy(null)
    }
  }, [ksId, loadDecisions, onChanged])

  const counts = result?.counts
  const violations = result?.violations ?? []
  const rows: Row[] = [
    ...violations.map<Row>((v) => ({
      key: `v${v.id}`, pending: true, type: TYPE_LABEL[v.type], subject: v.summary,
      status: v.severity, statusTone: v.severity === "error" ? "error" : "warning",
      reason: null, by: null, when: null, violation: v,
    })),
    ...decisions.map<Row>((d) => ({
      key: `d${d.id}`, pending: false, type: "Datatype rule",
      subject: d.property_label + (d.xsd_type ? `  ·  xsd:${d.xsd_type}` : ""),
      status: d.action === "relax" ? "→ text" : "remove noise", statusTone: "neutral",
      reason: d.reason, by: d.resolved_by, when: d.created_at, onForget: () => forget(d),
    })),
  ]

  const term = q.trim().toLowerCase()
  const filtered = rows.filter((r) =>
    (filter === "all" || (filter === "pending" ? r.pending : !r.pending)) &&
    `${r.type} ${r.subject} ${r.status}`.toLowerCase().includes(term))
  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE))
  const p = Math.min(page, pageCount - 1)
  const shown = filtered.slice(p * PAGE_SIZE, p * PAGE_SIZE + PAGE_SIZE)

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-semibold">Validation</h2>
          <p className="text-xs text-muted-foreground">
            Individuals checked against the ontology's disjointness, domain/range and datatype constraints —
            fixes the agent learns become reusable rules.
          </p>
        </div>
        <div className="flex items-center gap-2">
          {counts && (counts.error > 0 || counts.warning > 0) && (
            <div className="flex items-center gap-1.5 text-xs">
              <Badge variant="outline" className="gap-1 text-destructive">{counts.error} error{counts.error === 1 ? "" : "s"}</Badge>
              <Badge variant="outline" className="gap-1 text-amber-600 dark:text-amber-400">{counts.warning} warning{counts.warning === 1 ? "" : "s"}</Badge>
            </div>
          )}
          <Button size="sm" variant="outline" onClick={load} disabled={loading} title="Re-check individuals">
            {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RefreshCw className="h-3.5 w-3.5" />} Re-run
          </Button>
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search…" className="h-8 w-44 pl-7 text-sm" />
          </div>
          <Select value={filter} onValueChange={(v) => setFilter(v as Filter)}>
            <SelectTrigger className="h-8 w-32 text-sm"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All</SelectItem>
              <SelectItem value="pending">Pending{violations.length ? ` (${violations.length})` : ""}</SelectItem>
              <SelectItem value="decided">Decided</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      {result?.truncated && (
        <p className="text-xs text-muted-foreground">Showing the first {violations.length} violations; fix some and re-run to see more.</p>
      )}

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
              <TableRow><TableCell colSpan={7} className="h-16 text-center text-muted-foreground">Checking individuals…</TableCell></TableRow>
            ) : shown.length === 0 ? (
              <TableRow><TableCell colSpan={7} className="h-16 text-center text-muted-foreground">No data</TableCell></TableRow>
            ) : shown.map((r) => (
              <TableRow key={r.key} className={r.pending ? "bg-amber-500/5" : undefined}>
                <TableCell><Badge variant="outline" className="text-[10px]">{r.type}</Badge></TableCell>
                <TableCell className="max-w-[20rem] truncate font-medium" title={r.subject}>{r.subject}</TableCell>
                <TableCell>
                  <Badge variant="outline" className={`text-[10px] ${
                    r.statusTone === "error" ? "text-destructive"
                      : r.statusTone === "warning" ? "text-amber-600 dark:text-amber-400" : ""}`}>
                    {r.status}
                  </Badge>
                </TableCell>
                <TableCell className="max-w-[16rem] truncate text-xs text-muted-foreground" title={r.reason ?? ""}>{r.reason || "—"}</TableCell>
                <TableCell><By by={r.by} /></TableCell>
                <TableCell className="text-xs text-muted-foreground" title={r.when ?? ""}>{fmtWhen(r.when)}</TableCell>
                <TableCell className="text-right">
                  {!canWrite ? null : r.pending && r.violation ? (
                    r.violation.fixes.length > 0 ? (
                      <FixMenu v={r.violation} busy={busy} onFix={applyFix} />
                    ) : null
                  ) : r.onForget ? (
                    <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      title="Forget this rule" disabled={busy !== null} onClick={r.onForget}>
                      <Trash2 className="h-3.5 w-3.5" />
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

/** Compact per-violation action: each available fix as a menu item. */
function FixMenu({
  v, busy, onFix,
}: {
  v: Violation
  busy: string | null
  onFix: (v: Violation, fix: ValidationFix) => void
}) {
  const active = busy !== null && busy.startsWith(v.id)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="sm" variant="outline" className="h-7 gap-1" disabled={active}>
          {active ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <>Fix <ChevronDown className="h-3.5 w-3.5" /></>}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="max-w-[18rem]">
        {v.fixes.map((fix) => (
          <DropdownMenuItem key={fix.id} onClick={() => onFix(v, fix)}>{fix.label}</DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
