import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { ChevronLeft, ChevronRight, FileText, Loader2, Plus, Search, Sparkles, Trash2, X } from "lucide-react"
import { api } from "@/lib/api"
import type { AboxClass, AboxSource, ExtractionJob, Individual, IndividualSummary, OntologyView } from "@/lib/types"
import ExtractDialog from "@/components/ExtractDialog"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Combobox } from "@/components/ui/combobox"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog"
import {
  Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle,
} from "@/components/ui/sheet"

const PAGE_SIZE = 20
const shortIri = (iri: string) => iri.split(/[#/]/).pop() ?? iri

/**
 * Instances (ABox) browser: pick a class on the left, page/search its individuals on the
 * right, click one to open a detail sheet with its type(s) and object/data assertions.
 * Editors can add/delete individuals and assertions. All writes go through the API and are
 * recorded in the change history (graph-scoped rollback).
 */
export default function InstancesPanel({
  ksId, view, canWrite, onChanged,
}: {
  ksId: number
  view: OntologyView
  canWrite: boolean
  onChanged?: () => void
}) {
  const [classes, setClasses] = useState<AboxClass[]>([])
  const [total, setTotal] = useState(0)
  const [selected, setSelected] = useState<string | null>(null) // class iri; null = all
  const [q, setQ] = useState("")
  const [debouncedQ, setDebouncedQ] = useState("")
  const [page, setPage] = useState(0)
  const [items, setItems] = useState<IndividualSummary[]>([])
  const [listTotal, setListTotal] = useState(0)
  const [loading, setLoading] = useState(true)

  const [openIri, setOpenIri] = useState<string | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [extractOpen, setExtractOpen] = useState(false)
  const [job, setJob] = useState<ExtractionJob | null>(null)
  const [classFilter, setClassFilter] = useState("")
  const [suggestions, setSuggestions] = useState<Record<string, number>>({})
  const [suggestJobId, setSuggestJobId] = useState<number | null>(null)
  const [dismissedJobId, setDismissedJobId] = useState<number | null>(null)
  const [addingClasses, setAddingClasses] = useState(false)

  const loadClasses = useCallback(async () => {
    try {
      const res = await api.aboxClasses(ksId)
      setClasses(res.classes)
      setTotal(res.total)
    } catch (e) {
      toast.error(`Failed to load classes: ${(e as Error).message}`)
    }
  }, [ksId])

  useEffect(() => { loadClasses() }, [loadClasses])

  useEffect(() => {
    const t = setTimeout(() => setDebouncedQ(q), 300)
    return () => clearTimeout(t)
  }, [q])
  useEffect(() => { setPage(0) }, [selected, debouncedQ])
  // Clamp the page when the list shrinks (e.g. deleting the last individual on the last page),
  // so we don't strand the user on an out-of-range page showing "No individuals yet".
  useEffect(() => { setPage((p) => Math.min(p, Math.max(0, Math.ceil(listTotal / PAGE_SIZE) - 1))) }, [listTotal])

  const loadItems = useCallback(async () => {
    setLoading(true)
    try {
      const res = await api.aboxIndividuals(ksId, {
        class_iri: selected ?? undefined,
        q: debouncedQ || undefined,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      })
      setItems(res.items)
      setListTotal(res.total)
    } catch (e) {
      toast.error(`Failed to load individuals: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [ksId, selected, debouncedQ, page])

  useEffect(() => { loadItems() }, [loadItems])

  // Classes the instance extractor referenced but that don't exist in the TBox yet — surfaced
  // from the latest extraction job as "add these to your ontology" suggestions. Labels that
  // already exist in the ontology (e.g. just added) are filtered out, so the banner doesn't
  // re-appear for classes the user has already adopted.
  const loadSuggestions = useCallback(async () => {
    try {
      const jobs = await api.listJobs(ksId)
      const j = jobs.find(
        (x) => x.status === "completed" && (x.kind === "abox" || x.kind === "both")
          && Object.keys(x.unknown_classes || {}).length > 0,
      )
      if (!j) { setSuggestions({}); setSuggestJobId(null); return }
      const existing = new Set(view.classes.map((c) => c.label.trim().toLowerCase()))
      const filtered = Object.fromEntries(
        Object.entries(j.unknown_classes).filter(([label]) => !existing.has(label.trim().toLowerCase())),
      )
      setSuggestions(filtered)
      setSuggestJobId(j.id)
    } catch { /* ignore */ }
  }, [ksId, view])

  useEffect(() => { loadSuggestions() }, [loadSuggestions])

  // Resume polling an ABox job left running (e.g. the user navigated away and back while it ran).
  useEffect(() => {
    let cancelled = false
    api.listJobs(ksId).then((jobs) => {
      if (cancelled) return
      const running = jobs.find((x) => (x.status === "running" || x.status === "pending") && x.kind === "abox")
      if (running) setJob((prev) => prev ?? running)
    }).catch(() => {})
    return () => { cancelled = true }
  }, [ksId])

  // Refresh everything after a mutation (counts + list + suggestions + parent history/overview).
  const refreshAll = useCallback(() => {
    loadClasses()
    loadItems()
    loadSuggestions()
    onChanged?.()
  }, [loadClasses, loadItems, loadSuggestions, onChanged])

  const addSuggestedClasses = useCallback(async () => {
    const labels = Object.keys(suggestions)
    setAddingClasses(true)
    let ok = 0
    for (const label of labels) {
      try { await api.editOntology(ksId, { op: "add_class", label }); ok++ } catch { /* skip dups */ }
    }
    setAddingClasses(false)
    toast.success(`Added ${ok} class(es) to the ontology — re-extract to type these instances`)
    setSuggestions({})
    setDismissedJobId(suggestJobId)
    onChanged?.()
  }, [ksId, suggestions, suggestJobId, onChanged])

  // Poll the ABox extraction job until it finishes, then refresh.
  useEffect(() => {
    if (!job) return
    if (job.status === "completed" || job.status === "failed") {
      if (job.status === "completed") {
        toast.success(
          `Instances extracted: +${job.individuals_added} new / ${job.pending_added} queued / ${job.assertions_added} assertions`,
        )
      } else {
        toast.error(`Instance extraction failed: ${job.error ?? "Unknown error"}`)
      }
      setJob(null)
      refreshAll()
      return
    }
    const t = setTimeout(async () => {
      try {
        setJob(await api.getJob(ksId, job.id))
      } catch {
        setJob(null)
      }
    }, 1500)
    return () => clearTimeout(t)
  }, [job, ksId, refreshAll])

  const pageCount = Math.max(1, Math.ceil(listTotal / PAGE_SIZE))
  const selectedLabel = selected ? classes.find((c) => c.iri === selected)?.label ?? shortIri(selected) : "All individuals"
  const extracting = job != null && (job.status === "running" || job.status === "pending")
  const suggestionLabels = Object.keys(suggestions)
  const showSuggestions = suggestionLabels.length > 0 && suggestJobId !== dismissedJobId

  return (
    <div className="space-y-3">
      {showSuggestions && (
        <div className="rounded-lg border border-amber-500/40 bg-amber-500/5 p-3">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">
                Instances referenced {suggestionLabels.length} class{suggestionLabels.length === 1 ? "" : "es"} not in your ontology
              </p>
              <p className="mt-0.5 text-xs text-muted-foreground">
                {suggestionLabels.slice(0, 12).join("、")}{suggestionLabels.length > 12 ? " …" : ""} — add them to the schema, then re-extract to type these instances.
              </p>
            </div>
            <div className="flex shrink-0 items-center gap-1.5">
              {canWrite && (
                <Button size="sm" onClick={addSuggestedClasses} disabled={addingClasses}>
                  {addingClasses ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Plus className="h-3.5 w-3.5" />}
                  Add {suggestionLabels.length} to ontology
                </Button>
              )}
              <Button size="icon" variant="ghost" className="h-7 w-7" onClick={() => setDismissedJobId(suggestJobId)}>
                <X className="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>
        </div>
      )}

      <div className="flex gap-4">
      {/* Left: classes with instance counts */}
      <div className="w-56 shrink-0 space-y-2">
        <div className="flex items-center justify-between px-1">
          <span className="text-xs font-medium text-muted-foreground">Classes</span>
          <span className="text-xs text-muted-foreground">{total}</span>
        </div>
        {classes.length > 8 && (
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input value={classFilter} onChange={(e) => setClassFilter(e.target.value)} placeholder="Filter classes…" className="h-8 pl-7 text-sm" />
          </div>
        )}
        <ScrollArea className="h-[calc(100svh-13rem)] pr-2">
          <div className="space-y-0.5">
            <ClassRow label="All individuals" count={total} active={selected === null} onClick={() => setSelected(null)} />
            {classes.filter((c) => c.label.toLowerCase().includes(classFilter.trim().toLowerCase())).map((c) => (
              <ClassRow
                key={c.iri} label={c.label} count={c.count}
                active={selected === c.iri} onClick={() => setSelected(c.iri)}
              />
            ))}
          </div>
        </ScrollArea>
      </div>

      {/* Right: individuals table */}
      <div className="min-w-0 flex-1 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="min-w-0">
            <h2 className="truncate text-sm font-semibold" title={selectedLabel}>{selectedLabel}</h2>
            <p className="text-xs text-muted-foreground">{listTotal} individual{listTotal === 1 ? "" : "s"}</p>
          </div>
          <div className="flex items-center gap-2">
            <div className="relative">
              <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={q} onChange={(e) => setQ(e.target.value)}
                placeholder="Search individuals…" className="h-8 w-52 pl-7 text-sm"
              />
            </div>
            {canWrite && (
              <Button size="sm" variant="outline" onClick={() => setExtractOpen(true)} disabled={extracting}>
                {extracting ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Sparkles className="h-3.5 w-3.5" />}
                Extract from documents
              </Button>
            )}
            {canWrite && (
              <Button size="sm" variant="outline" onClick={() => setCreateOpen(true)}>
                <Plus className="h-3.5 w-3.5" /> Add individual
              </Button>
            )}
          </div>
        </div>

        {extracting && (
          <div className="space-y-1 rounded-md border bg-muted/30 px-3 py-2">
            <div className="flex items-center justify-between text-xs text-muted-foreground">
              <span>Extracting instances… {job!.processed_chunks}/{job!.total_chunks} chunks · {job!.model}</span>
            </div>
            <div className="h-1.5 overflow-hidden rounded-full bg-muted">
              <div
                className="h-full rounded-full bg-primary transition-all"
                style={{ width: `${job!.total_chunks ? Math.max(6, (job!.processed_chunks / job!.total_chunks) * 100) : 6}%` }}
              />
            </div>
          </div>
        )}

        <div className="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Label</TableHead>
                <TableHead>Type</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {loading ? (
                <TableRow><TableCell colSpan={2} className="h-24 text-center text-muted-foreground">Loading…</TableCell></TableRow>
              ) : items.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={2} className="h-24 text-center text-muted-foreground">
                    {debouncedQ ? "No matching individuals." : "No individuals yet."}
                  </TableCell>
                </TableRow>
              ) : (
                items.map((i) => (
                  <TableRow key={i.iri} className="cursor-pointer" onClick={() => setOpenIri(i.iri)}>
                    <TableCell className="font-medium">{i.label}</TableCell>
                    <TableCell className="text-muted-foreground">
                      {i.types.length ? i.types.map((t) => t.label).join(", ") : "—"}
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>

        {listTotal > PAGE_SIZE && (
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>{page * PAGE_SIZE + 1}–{Math.min(listTotal, (page + 1) * PAGE_SIZE)} of {listTotal}</span>
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

      {createOpen && (
        <CreateDialog
          ksId={ksId} view={view} defaultClass={selected}
          onClose={() => setCreateOpen(false)}
          onCreated={(ind) => { setCreateOpen(false); refreshAll(); setOpenIri(ind.iri) }}
        />
      )}

      <ExtractDialog
        ksId={ksId} mode="abox" open={extractOpen} onOpenChange={setExtractOpen}
        onStarted={(j) => setJob(j)}
      />

      {openIri && (
        <IndividualSheet
          ksId={ksId} iri={openIri} view={view} canWrite={canWrite}
          onClose={() => setOpenIri(null)}
          onChanged={refreshAll}
          onDeleted={() => { setOpenIri(null); refreshAll() }}
        />
      )}
      </div>
    </div>
  )
}

function ClassRow({ label, count, active, onClick }: { label: string; count: number; active: boolean; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className={`flex w-full items-center justify-between gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors ${
        active ? "bg-accent text-accent-foreground" : "hover:bg-accent/50"
      }`}
    >
      <span className="truncate" title={label}>{label}</span>
      <span className="shrink-0 text-xs text-muted-foreground">{count}</span>
    </button>
  )
}

function CreateDialog({
  ksId, view, defaultClass, onClose, onCreated,
}: {
  ksId: number
  view: OntologyView
  defaultClass: string | null
  onClose: () => void
  onCreated: (ind: Individual) => void
}) {
  const [label, setLabel] = useState("")
  const [classIri, setClassIri] = useState(defaultClass ?? view.classes[0]?.iri ?? "")
  const [saving, setSaving] = useState(false)

  const submit = async () => {
    if (!label.trim() || !classIri) return
    setSaving(true)
    try {
      const ind = await api.createIndividual(ksId, label.trim(), classIri)
      toast.success(`Added "${ind.label}"`)
      onCreated(ind)
    } catch (e) {
      toast.error(`Create failed: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add individual</DialogTitle>
          <DialogDescription>Create a new instance of a class.</DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1.5">
            <Label>Label</Label>
            <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="e.g. Well No. 3" autoFocus
              onKeyDown={(e) => e.key === "Enter" && submit()} />
          </div>
          <div className="space-y-1.5">
            <Label>Class</Label>
            <Combobox
              value={classIri} onChange={setClassIri}
              options={view.classes.map((c) => ({ value: c.iri, label: c.label }))}
              placeholder="Select a class" searchPlaceholder="Search classes…"
            />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          <Button onClick={submit} disabled={saving || !label.trim() || !classIri}>
            {saving && <Loader2 className="h-4 w-4 animate-spin" />} Create
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function IndividualSheet({
  ksId, iri, view, canWrite, onClose, onChanged, onDeleted,
}: {
  ksId: number
  iri: string
  view: OntologyView
  canWrite: boolean
  onClose: () => void
  onChanged: () => void
  onDeleted: () => void
}) {
  const [ind, setInd] = useState<Individual | null>(null)
  const [loading, setLoading] = useState(true)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      setInd(await api.getIndividual(ksId, iri))
    } catch (e) {
      toast.error(`Failed to load: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [ksId, iri])

  useEffect(() => { load() }, [load])

  const afterMutation = async () => { await load(); onChanged() }

  const removeObject = async (prop: string, target: string) => {
    try {
      await api.removeAssertion(ksId, { subject: iri, prop, kind: "object", target })
      afterMutation()
    } catch (e) { toast.error((e as Error).message.replace(/^\d+:\s*/, "")) }
  }
  const removeData = async (prop: string, value: string, datatype: string | null) => {
    try {
      await api.removeAssertion(ksId, { subject: iri, prop, kind: "data", value, datatype })
      afterMutation()
    } catch (e) { toast.error((e as Error).message.replace(/^\d+:\s*/, "")) }
  }
  const del = async () => {
    if (!confirm(`Delete individual "${ind?.label}" and all its assertions?`)) return
    try {
      await api.deleteIndividual(ksId, iri)
      toast.success("Individual deleted")
      onDeleted()
    } catch (e) { toast.error((e as Error).message.replace(/^\d+:\s*/, "")) }
  }

  return (
    <Sheet open onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="w-full overflow-y-auto sm:max-w-lg">
        <SheetHeader>
          <SheetTitle className="truncate">{loading ? "Loading…" : ind?.label}</SheetTitle>
          <SheetDescription className="break-all font-mono text-[11px]">{iri}</SheetDescription>
        </SheetHeader>

        {ind && (
          <div className="space-y-5 px-4 pb-8">
            <section>
              <h4 className="mb-1.5 text-xs font-medium text-muted-foreground">Types</h4>
              <div className="flex flex-wrap gap-1.5">
                {ind.types.length ? ind.types.map((t) => (
                  <Badge key={t.iri} variant="secondary">{t.label}</Badge>
                )) : <span className="text-sm text-muted-foreground">—</span>}
              </div>
            </section>

            <SourceSection sources={ind.sources ?? []} />

            <AssertionList
              title="Relationships"
              rows={ind.object_assertions.map((a) => ({
                key: `${a.prop}|${a.target}`, prop: a.prop_label, value: a.target_label, sources: a.sources,
                onRemove: canWrite ? () => removeObject(a.prop, a.target) : undefined,
              }))}
            />
            <AssertionList
              title="Attributes"
              rows={ind.data_assertions.map((a) => ({
                key: `${a.prop}|${a.value}`, prop: a.prop_label, value: a.value, sources: a.sources,
                onRemove: canWrite ? () => removeData(a.prop, a.value, a.datatype) : undefined,
              }))}
            />

            {canWrite && (
              <AddAssertion ksId={ksId} subject={iri} view={view} onAdded={afterMutation} />
            )}

            {canWrite && (
              <div className="border-t pt-4">
                <Button variant="ghost" className="text-destructive hover:text-destructive" onClick={del}>
                  <Trash2 className="h-4 w-4" /> Delete individual
                </Button>
              </div>
            )}
          </div>
        )}
      </SheetContent>
    </Sheet>
  )
}

/** The distinct source documents an individual (or assertion) was extracted from, each with a
 *  text snippet — the "溯源" for instance data. */
function docName(s: AboxSource) {
  return s.document ?? `文档 #${s.document_id ?? "?"}`
}
function SourceSection({ sources }: { sources: AboxSource[] }) {
  if (sources.length === 0) return null
  // De-dup by document (an individual is usually mentioned across a few chunks of the same doc).
  const byDoc = new Map<string, AboxSource>()
  for (const s of sources) {
    const k = String(s.document_id ?? `c${s.chunk_id}`)
    if (!byDoc.has(k)) byDoc.set(k, s)
  }
  return (
    <section>
      <h4 className="mb-1.5 text-xs font-medium text-muted-foreground">Sources · 来源</h4>
      <ul className="space-y-1.5">
        {[...byDoc.values()].map((s, i) => (
          <li key={i} className="rounded-md border px-2.5 py-1.5">
            <div className="flex items-center gap-1.5 text-xs font-medium">
              <FileText className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <span className="truncate" title={docName(s)}>{docName(s)}</span>
            </div>
            {s.snippet && <p className="mt-1 line-clamp-3 text-xs text-muted-foreground">{s.snippet}</p>}
          </li>
        ))}
      </ul>
    </section>
  )
}

function AssertionList({
  title, rows,
}: {
  title: string
  rows: { key: string; prop: string; value: string; sources?: AboxSource[]; onRemove?: () => void }[]
}) {
  return (
    <section>
      <h4 className="mb-1.5 text-xs font-medium text-muted-foreground">{title}</h4>
      {rows.length === 0 ? (
        <p className="text-sm text-muted-foreground">—</p>
      ) : (
        <ul className="space-y-1">
          {rows.map(({ key, ...r }) => <AssertionRow key={key} {...r} />)}
        </ul>
      )}
    </section>
  )
}

function AssertionRow({
  prop, value, sources, onRemove,
}: {
  prop: string; value: string; sources?: AboxSource[]; onRemove?: () => void
}) {
  const [open, setOpen] = useState(false)
  const n = sources?.length ?? 0
  return (
    <li className="rounded-md border px-2.5 py-1.5 text-sm">
      <div className="flex items-center justify-between gap-2">
        <span className="min-w-0 truncate">
          <span className="text-muted-foreground">{prop}</span>{" "}
          <span className="font-medium">{value}</span>
        </span>
        <div className="flex shrink-0 items-center gap-0.5">
          {n > 0 && (
            <button
              type="button" onClick={() => setOpen((o) => !o)} title={`${n} 处来源`}
              className={`inline-flex items-center gap-0.5 rounded px-1 py-0.5 text-[11px] ${open ? "text-primary" : "text-muted-foreground hover:text-foreground"}`}
            >
              <FileText className="h-3.5 w-3.5" />{n}
            </button>
          )}
          {onRemove && (
            <Button size="icon" variant="ghost" className="h-6 w-6 text-muted-foreground hover:text-destructive" onClick={onRemove}>
              <X className="h-3.5 w-3.5" />
            </Button>
          )}
        </div>
      </div>
      {open && n > 0 && (
        <div className="mt-1.5 space-y-1.5 border-t pt-1.5">
          {sources!.map((s, i) => (
            <div key={i} className="text-xs">
              <div className="flex items-center gap-1.5 font-medium text-muted-foreground">
                <FileText className="h-3 w-3 shrink-0" /><span className="truncate" title={docName(s)}>{docName(s)}</span>
              </div>
              {s.snippet && <p className="mt-0.5 line-clamp-3 text-muted-foreground">{s.snippet}</p>}
            </div>
          ))}
        </div>
      )}
    </li>
  )
}

function AddAssertion({
  ksId, subject, view, onAdded,
}: {
  ksId: number
  subject: string
  view: OntologyView
  onAdded: () => void
}) {
  const [kind, setKind] = useState<"object" | "data">("object")
  const [prop, setProp] = useState("")
  const [target, setTarget] = useState("")
  const [value, setValue] = useState("")
  const [saving, setSaving] = useState(false)
  const [targets, setTargets] = useState<IndividualSummary[]>([])

  const props = kind === "object" ? view.object_properties : view.data_properties

  // Load candidate object targets lazily (up to 200) when object kind is chosen.
  useEffect(() => {
    if (kind !== "object") return
    let cancelled = false
    api.aboxIndividuals(ksId, { limit: 200 })
      .then((r) => { if (!cancelled) setTargets(r.items.filter((i) => i.iri !== subject)) })
      .catch(() => {})
    return () => { cancelled = true }
  }, [ksId, kind, subject])

  useEffect(() => { setProp(""); setTarget(""); setValue("") }, [kind])

  const canSubmit = prop && (kind === "object" ? target : value.trim())

  const submit = async () => {
    if (!canSubmit) return
    setSaving(true)
    try {
      await api.addAssertion(ksId, kind === "object"
        ? { subject, prop, kind, target }
        : { subject, prop, kind, value: value.trim() })
      setProp(""); setTarget(""); setValue("")
      onAdded()
    } catch (e) {
      toast.error(`Add failed: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    } finally {
      setSaving(false)
    }
  }

  return (
    <section className="rounded-lg border bg-muted/30 p-3">
      <h4 className="mb-2 text-xs font-medium text-muted-foreground">Add assertion</h4>
      <div className="space-y-2">
        <div className="flex gap-2">
          <Select value={kind} onValueChange={(v) => setKind(v as "object" | "data")}>
            <SelectTrigger className="h-8 w-32 text-sm"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="object">Relationship</SelectItem>
              <SelectItem value="data">Attribute</SelectItem>
            </SelectContent>
          </Select>
          <Combobox
            value={prop} onChange={setProp} className="flex-1" triggerClassName="h-8"
            options={props.map((p) => ({ value: p.iri, label: p.label }))}
            placeholder="Property" searchPlaceholder="Search properties…"
            emptyText={`No ${kind} properties defined`}
          />
        </div>
        {kind === "object" ? (
          <Combobox
            value={target} onChange={setTarget} triggerClassName="h-8"
            options={targets.map((t) => ({ value: t.iri, label: t.label }))}
            placeholder="Target individual" searchPlaceholder="Search individuals…"
            emptyText="No other individuals"
          />
        ) : (
          <Input value={value} onChange={(e) => setValue(e.target.value)} placeholder="Value"
            className="h-8 text-sm" onKeyDown={(e) => e.key === "Enter" && submit()} />
        )}
        <div className="flex justify-end">
          <Button size="sm" onClick={submit} disabled={!canSubmit || saving}>
            {saving ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Plus className="h-3.5 w-3.5" />} Add
          </Button>
        </div>
      </div>
    </section>
  )
}
