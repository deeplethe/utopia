import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { ChevronLeft, ChevronRight, Loader2, RotateCcw, Search } from "lucide-react"
import { api } from "@/lib/api"
import type { AuditEvent } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"

const PAGE_SIZE = 20

const CATEGORIES = [
  { value: "all", label: "All events" },
  { value: "ontology", label: "Ontology edits" },
  { value: "abox", label: "Instances" },
  { value: "conflict", label: "Conflicts" },
  { value: "extraction", label: "Extraction" },
  { value: "document", label: "Documents" },
  { value: "member", label: "Members" },
  { value: "ks", label: "Settings" },
]

// Category label + badge tone from the action prefix (e.g. "ontology.edit" -> "ontology").
function categoryOf(action: string) {
  return action.split(".")[0]
}
const CAT_LABEL: Record<string, string> = {
  ontology: "Ontology", abox: "Instance", conflict: "Conflict", extraction: "Extraction",
  document: "Document", member: "Member", ks: "Settings", system: "Rollback",
}

export default function KsHistory({
  ksId, canWrite, onChanged,
}: {
  ksId: number
  canWrite: boolean
  onChanged?: () => void
}) {
  const [category, setCategory] = useState("all")
  const [q, setQ] = useState("")
  const [debouncedQ, setDebouncedQ] = useState("")
  const [page, setPage] = useState(0)
  const [items, setItems] = useState<AuditEvent[]>([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(true)
  const [rollingBack, setRollingBack] = useState<number | null>(null)

  // Debounce the keyword search.
  useEffect(() => {
    const t = setTimeout(() => setDebouncedQ(q), 300)
    return () => clearTimeout(t)
  }, [q])

  useEffect(() => { setPage(0) }, [category, debouncedQ])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const res = await api.getHistory(ksId, {
        category: category === "all" ? undefined : category,
        q: debouncedQ || undefined,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      })
      setItems(res.items)
      setTotal(res.total)
    } catch (e) {
      toast.error(`Failed to load history: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [ksId, category, debouncedQ, page])

  useEffect(() => { load() }, [load])

  const rollback = useCallback(async (ev: AuditEvent) => {
    if (!confirm(
      `Roll back to before this change?\n\n"${ev.summary}"\n\nThis undoes this change and everything after it in the ontology. It can itself be undone from history.`,
    )) return
    setRollingBack(ev.id)
    try {
      const res = await api.rollbackHistory(ksId, ev.id)
      toast.success(`Rolled back — undid ${res.undone} change(s)`)
      onChanged?.()
      load()
    } catch (e) {
      toast.error(`Rollback failed: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    } finally {
      setRollingBack(null)
    }
  }, [ksId, onChanged, load])

  const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE))

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-semibold">Change history</h2>
          <p className="text-xs text-muted-foreground">Every change to this knowledge system, most recent first.</p>
        </div>
        <div className="flex items-center gap-2">
          <Select value={category} onValueChange={setCategory}>
            <SelectTrigger className="h-8 w-40 text-sm"><SelectValue /></SelectTrigger>
            <SelectContent>
              {CATEGORIES.map((c) => <SelectItem key={c.value} value={c.value}>{c.label}</SelectItem>)}
            </SelectContent>
          </Select>
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={q} onChange={(e) => setQ(e.target.value)}
              placeholder="Search summary or actor…" className="h-8 w-56 pl-7 text-sm"
            />
          </div>
        </div>
      </div>

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-44">Time</TableHead>
              <TableHead className="w-32">Actor</TableHead>
              <TableHead className="w-28">Category</TableHead>
              <TableHead>Event</TableHead>
              {canWrite && <TableHead className="w-24 text-right">Rollback</TableHead>}
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={canWrite ? 5 : 4} className="h-20 text-center text-muted-foreground">Loading…</TableCell></TableRow>
            ) : items.length === 0 ? (
              <TableRow><TableCell colSpan={canWrite ? 5 : 4} className="h-20 text-center text-muted-foreground">
                {debouncedQ || category !== "all" ? "No matching events." : "No changes recorded yet."}
              </TableCell></TableRow>
            ) : (
              items.map((ev) => (
                <TableRow key={ev.id}>
                  <TableCell className="whitespace-nowrap text-xs text-muted-foreground">
                    {new Date(ev.created_at).toLocaleString()}
                  </TableCell>
                  <TableCell className="font-medium">{ev.actor_name}</TableCell>
                  <TableCell>
                    <Badge variant="secondary" className="text-[10px]">{CAT_LABEL[categoryOf(ev.action)] ?? ev.action}</Badge>
                  </TableCell>
                  <TableCell>{ev.summary}</TableCell>
                  {canWrite && (
                    <TableCell className="text-right">
                      {ev.can_rollback && (
                        <Button
                          size="sm" variant="ghost" className="h-7 gap-1 text-xs"
                          disabled={rollingBack !== null}
                          onClick={() => rollback(ev)}
                          title="Roll back the ontology to before this change"
                        >
                          {rollingBack === ev.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RotateCcw className="h-3.5 w-3.5" />}
                          Revert
                        </Button>
                      )}
                    </TableCell>
                  )}
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>

      {total > PAGE_SIZE && (
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>{page * PAGE_SIZE + 1}–{Math.min(total, (page + 1) * PAGE_SIZE)} of {total}</span>
          <div className="flex gap-1">
            <Button size="sm" variant="outline" className="h-7 w-7 p-0" disabled={page === 0} onClick={() => setPage(page - 1)}>
              <ChevronLeft className="h-4 w-4" />
            </Button>
            <Button size="sm" variant="outline" className="h-7 w-7 p-0" disabled={page >= pageCount - 1} onClick={() => setPage(page + 1)}>
              <ChevronRight className="h-4 w-4" />
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}
