import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { ChevronLeft, ChevronRight, Loader2, RotateCcw, Search } from "lucide-react"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type { AuditEvent } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"

const PAGE_SIZE = 20

// Category label + badge tone from the action prefix (e.g. "ontology.edit" -> "ontology").
function categoryOf(action: string) {
  return action.split(".")[0]
}

export default function KsHistory({
  ksId, canWrite, onChanged,
}: {
  ksId: number
  canWrite: boolean
  onChanged?: () => void
}) {
  const { locale, t } = useI18n()
  const confirmAction = useConfirm()
  const categories = [
    { value: "all", label: t("history.allEvents") },
    { value: "ontology", label: t("history.ontologyEdits") },
    { value: "abox", label: t("history.instances") },
    { value: "conflict", label: t("history.conflicts") },
    { value: "extraction", label: t("history.extraction") },
    { value: "rdf", label: t("history.rdfImports") },
    { value: "document", label: t("history.documents") },
    { value: "member", label: t("history.members") },
    { value: "token", label: t("history.apiAccess") },
    { value: "prompt", label: t("history.prompts") },
    { value: "ks", label: t("history.settings") },
  ]
  const categoryLabels: Record<string, string> = {
    ontology: t("audit.ontology"), abox: t("audit.instance"), conflict: t("audit.conflict"),
    extraction: t("audit.extraction"), rdf: t("audit.rdfImport"), document: t("audit.document"),
    member: t("audit.member"), token: t("audit.apiAccess"), prompt: t("audit.prompt"),
    ks: t("audit.settings"), system: t("audit.rollback"),
  }
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
    const timer = setTimeout(() => setDebouncedQ(q), 300)
    return () => clearTimeout(timer)
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
      toast.error(t("history.loadFailed", { error: (e as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [ksId, category, debouncedQ, page, t])

  useEffect(() => { load() }, [load])

  const rollback = useCallback(async (ev: AuditEvent) => {
    if (!await confirmAction(t("history.rollbackConfirm", { summary: ev.summary }), { destructive: true })) return
    setRollingBack(ev.id)
    try {
      const res = await api.rollbackHistory(ksId, ev.id)
      toast.success(t("history.rolledBack", { count: res.undone }))
      onChanged?.()
      load()
    } catch (e) {
      toast.error(t("history.rollbackFailed", { error: (e as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setRollingBack(null)
    }
  }, [confirmAction, ksId, onChanged, load, t])

  const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE))

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-semibold">{t("history.title")}</h2>
        </div>
        <div className="flex items-center gap-2">
          <Select value={category} onValueChange={setCategory}>
            <SelectTrigger className="h-8 w-40 text-sm"><SelectValue /></SelectTrigger>
            <SelectContent>
              {categories.map((c) => <SelectItem key={c.value} value={c.value}>{c.label}</SelectItem>)}
            </SelectContent>
          </Select>
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={q} onChange={(e) => setQ(e.target.value)}
              placeholder={t("history.search")} className="h-8 w-56 pl-7 text-sm"
            />
          </div>
        </div>
      </div>

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-44">{t("history.time")}</TableHead>
              <TableHead className="w-32">{t("history.actor")}</TableHead>
              <TableHead className="w-28">{t("history.category")}</TableHead>
              <TableHead>{t("history.event")}</TableHead>
              {canWrite && <TableHead className="w-24 text-right">{t("history.rollback")}</TableHead>}
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={canWrite ? 5 : 4} className="h-20 text-center text-muted-foreground">{t("common.loading")}</TableCell></TableRow>
            ) : items.length === 0 ? (
              <TableRow><TableCell colSpan={canWrite ? 5 : 4} className="h-20 text-center text-muted-foreground">
                {debouncedQ || category !== "all" ? t("history.noMatches") : t("history.empty")}
              </TableCell></TableRow>
            ) : (
              items.map((ev) => (
                <TableRow key={ev.id}>
                  <TableCell className="whitespace-nowrap text-xs text-muted-foreground">
                    {new Date(ev.created_at).toLocaleString(locale)}
                  </TableCell>
                  <TableCell className="font-medium">{ev.actor_name}</TableCell>
                  <TableCell>
                    <Badge variant="secondary" className="text-[10px]">{categoryLabels[categoryOf(ev.action)] ?? ev.action}</Badge>
                  </TableCell>
                  <TableCell>{ev.summary}</TableCell>
                  {canWrite && (
                    <TableCell className="text-right">
                      {ev.can_rollback && (
                        <Button
                          size="sm" variant="ghost" className="h-7 gap-1 text-xs"
                          disabled={rollingBack !== null}
                          onClick={() => rollback(ev)}
                          title={t("history.rollbackTitle")}
                        >
                          {rollingBack === ev.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RotateCcw className="h-3.5 w-3.5" />}
                          {t("history.revert")}
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
          <span>{t("review.page", { start: page * PAGE_SIZE + 1, end: Math.min(total, (page + 1) * PAGE_SIZE), total })}</span>
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
