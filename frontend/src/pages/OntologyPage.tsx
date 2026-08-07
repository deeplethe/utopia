import { useCallback, useEffect, useRef, useState } from "react"
import { Navigate, useParams, useSearchParams } from "react-router-dom"
import { toast } from "sonner"
import { ChevronDown, Crown, Download, Eye, Loader2, RefreshCw, Shield, ShieldAlert, Sparkles } from "lucide-react"
import { api } from "@/lib/api"
import type { Conflict, EditOp, EditResult, ExtractionJob, KnowledgeSystem, OntologyProperty, OntologyView, Role, SourceDoc } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import OntologyWorkbench from "@/components/OntologyWorkbench"
import ExtractDialog from "@/components/ExtractDialog"
import InstancesPanel from "@/components/InstancesPanel"
import ReviewPanel from "@/components/ReviewPanel"
import KsDocuments from "@/components/KsDocuments"
import KsHistory from "@/components/KsHistory"
import KsOverview from "@/components/KsOverview"
import MembersPanel from "@/components/MembersPanel"
import { AxiomDialog, ClassDialog, PropertyDialog } from "@/components/EditDialogs"

function RoleTag({ role }: { role: Role }) {
  if (role === "owner") return <Badge variant="outline" className="gap-1"><Crown className="h-3 w-3" /> Owner</Badge>
  if (role === "editor") return <Badge variant="outline" className="gap-1"><Shield className="h-3 w-3" /> Editor</Badge>
  return <Badge variant="outline" className="gap-1"><Eye className="h-3 w-3" /> Viewer</Badge>
}

type PropWithKind = OntologyProperty & { kind: "object" | "data" }

// Downloadable RDF serializations offered by the Export menu (must match backend EXPORT_FORMATS).
const EXPORT_FORMATS: { fmt: string; ext: string; label: string }[] = [
  { fmt: "turtle", ext: "ttl", label: "Turtle (.ttl)" },
  { fmt: "rdfxml", ext: "rdf", label: "RDF/XML (.rdf)" },
  { fmt: "ntriples", ext: "nt", label: "N-Triples (.nt)" },
  { fmt: "jsonld", ext: "jsonld", label: "JSON-LD (.jsonld)" },
]

export default function OntologyPage() {
  const { id, section: sectionParam, sub: subParam } = useParams()
  const [searchParams] = useSearchParams()
  const ksId = Number(id)
  const section = sectionParam ?? "overview"
  const REVIEW_SUBS = ["conflicts", "resolution", "validation"]
  const reviewSub = subParam && REVIEW_SUBS.includes(subParam) ? subParam : "conflicts"
  const [ks, setKs] = useState<KnowledgeSystem | null>(null)
  const [view, setView] = useState<OntologyView | null>(null)
  const [jobs, setJobs] = useState<ExtractionJob[]>([])
  const [sources, setSources] = useState<SourceDoc[]>([])
  const [conflicts, setConflicts] = useState<Conflict[]>([])
  const [loading, setLoading] = useState(true)
  const [extractOpen, setExtractOpen] = useState(false)
  const [activeJob, setActiveJob] = useState<ExtractionJob | null>(null)

  const [classDialog, setClassDialog] = useState<{ open: boolean; initial: OntologyView["classes"][number] | null }>({ open: false, initial: null })
  const [propDialog, setPropDialog] = useState<{ open: boolean; initial: PropWithKind | null }>({ open: false, initial: null })
  const [axiomOpen, setAxiomOpen] = useState(false)

  const refresh = useCallback(async () => {
    try {
      const [k, v, j, c, s] = await Promise.all([
        api.getKS(ksId), api.getOntology(ksId), api.listJobs(ksId), api.listConflicts(ksId), api.getSources(ksId),
      ])
      setKs(k); setView(v); setJobs(j); setConflicts(c); setSources(s)
    } catch (e) {
      toast.error(`Failed to load: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [ksId])

  useEffect(() => { refresh() }, [refresh])

  // Ids of jobs this page already finished, so the adopt effect below never re-adopts a job
  // whose completion we just handled (the `jobs` array is briefly stale after refresh()).
  const finishedJobs = useRef<Set<number>>(new Set())

  // Adopt a still-running job after a page (re)load — progress is persisted server-side,
  // so a refresh never loses the in-progress extraction.
  useEffect(() => {
    if (activeJob) return
    // Only TBox jobs are owned here; ABox (instance) jobs are polled inside InstancesPanel.
    const running = jobs.find((j) =>
      (j.status === "running" || j.status === "pending") && j.kind !== "abox" && !finishedJobs.current.has(j.id))
    if (running) setActiveJob(running)
  }, [jobs, activeJob])

  // Poll the active job until it finishes, then refresh + notify.
  useEffect(() => {
    if (!activeJob) return
    if (activeJob.status === "completed" || activeJob.status === "failed") {
      finishedJobs.current.add(activeJob.id)
      if (activeJob.status === "completed")
        toast.success(
          activeJob.kind === "both"
            ? `Extraction complete: +${activeJob.classes_added} classes / +${activeJob.properties_added} properties · +${activeJob.individuals_added} individuals / ${activeJob.pending_added} queued`
            : `Extraction complete: +${activeJob.classes_added} classes / +${activeJob.properties_added} properties / +${activeJob.axioms_added} axioms`,
        )
      else toast.error(`Extraction failed: ${activeJob.error ?? "Unknown error"}`)
      setActiveJob(null)
      refresh()
      return
    }
    const t = setTimeout(async () => {
      try {
        setActiveJob(await api.getJob(ksId, activeJob.id))
      } catch {
        setActiveJob(null)
      }
    }, 1500)
    return () => clearTimeout(t)
  }, [activeJob, ksId, refresh])

  const applyResult = (res: EditResult) => {
    setView(res.view)
    setConflicts(res.open_conflicts)
  }

  const runEdit = useCallback(async (op: EditOp, successMsg?: string) => {
    try {
      applyResult(await api.editOntology(ksId, op))
      if (successMsg) toast.success(successMsg)
    } catch (e) {
      toast.error(`Operation failed: ${(e as Error).message}`)
    }
  }, [ksId])

  const detect = useCallback(async () => {
    try {
      const c = await api.detectConflicts(ksId)
      setConflicts(c)
      toast[c.length ? "warning" : "success"](c.length ? `Found ${c.length} conflict(s)` : "No conflicts found")
    } catch (e) {
      toast.error(`Detection failed: ${(e as Error).message}`)
    }
  }, [ksId])

  const download = useCallback(async (fmt: string, ext: string) => {
    try {
      const data = await api.exportOntology(ksId, fmt)
      const url = URL.createObjectURL(new Blob([data], { type: "application/octet-stream" }))
      const a = document.createElement("a")
      a.href = url; a.download = `${ks?.name ?? "ontology"}.${ext}`; a.click()
      URL.revokeObjectURL(url)
    } catch (e) { toast.error(`Export failed: ${(e as Error).message}`) }
  }, [ksId, ks])

  const label = (iri: string) => view?.labels[iri] ?? iri.split(/[#/]/).pop() ?? iri

  // Graph + Classes & Properties are now one "Ontology" page (Graph / Table lenses); keep old
  // /graph, /entities and /axioms links working.
  if (section === "graph") return <Navigate to={`/knowledge/${ksId}/ontology`} replace />
  if (section === "entities" || section === "axioms") {
    const tab = section === "axioms" ? "axioms" : searchParams.get("tab")
    return <Navigate to={`/knowledge/${ksId}/ontology?view=table${tab ? `&tab=${tab}` : ""}`} replace />
  }
  // Turtle page retired — export moved to the multi-format Export menu on the overview.
  if (section === "turtle") return <Navigate to={`/knowledge/${ksId}/overview`} replace />
  // Review sub-panels are now second-level pages under /review/<sub>; keep old flat links working.
  if (["conflicts", "resolution", "validation"].includes(section))
    return <Navigate to={`/knowledge/${ksId}/review/${section}`} replace />
  if (section === "review" && !subParam) return <Navigate to={`/knowledge/${ksId}/review/conflicts`} replace />
  if (loading) return <p className="text-sm text-muted-foreground">Loading…</p>
  if (!ks || !view) return <p className="text-sm text-muted-foreground">Knowledge system not found.</p>

  const canWrite = ks.my_role === "editor" || ks.my_role === "owner"
  const canManage = ks.my_role === "owner"
  const errorCount = conflicts.filter((c) => c.severity === "error").length

  const axiomGroups = [
    {
      type: "Subclass",
      title: `Subclass subClassOf (${view.axioms.subclass_of.length})`,
      items: view.axioms.subclass_of.map((r) => ({
        text: `${label(r.sub)} ⊑ ${label(r.super)}`,
        parts: { left: label(r.sub), op: "sub" as const, right: label(r.super) },
        onDelete: canWrite ? () => runEdit({ op: "delete_axiom", type: "subclass", sub: r.sub, super: r.super }, "Deleted") : undefined,
      })),
    },
    {
      type: "Disjoint",
      title: `Disjoint disjointWith (${view.axioms.disjoint_with.length})`,
      items: view.axioms.disjoint_with.map((r) => ({
        text: `${label(r.a)} ⟂ ${label(r.b)}`,
        parts: { left: label(r.a), op: "disjoint" as const, right: label(r.b) },
        onDelete: canWrite ? () => runEdit({ op: "delete_axiom", type: "disjoint", a: r.a, b: r.b }, "Deleted") : undefined,
      })),
    },
    {
      type: "Equivalent",
      title: `Equivalent equivalentClass (${view.axioms.equivalent_class.length})`,
      items: view.axioms.equivalent_class.map((r) => ({
        text: `${label(r.a)} ≡ ${label(r.b)}`,
        parts: { left: label(r.a), op: "equiv" as const, right: label(r.b) },
        onDelete: canWrite ? () => runEdit({ op: "delete_axiom", type: "equivalent", a: r.a, b: r.b }, "Deleted") : undefined,
      })),
    },
  ]

  return (
    <div className="space-y-5">
      {section === "overview" && (
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="space-y-1">
          <div className="flex flex-wrap gap-2">
            <RoleTag role={ks.my_role} />
            <Badge variant="secondary">{view.stats.class_count} Classes</Badge>
            <Badge variant="secondary">{view.stats.property_count} Properties</Badge>
            <Badge variant="secondary">{view.stats.axiom_count} Axioms</Badge>
            {conflicts.length > 0 && (
              <Badge variant={errorCount ? "destructive" : "outline"} className="gap-1">
                <ShieldAlert className="h-3 w-3" /> {conflicts.length} Conflicts
              </Badge>
            )}
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" size="sm" onClick={refresh}><RefreshCw className="h-4 w-4" /> Refresh</Button>
          {canWrite && (
            <Button variant="outline" size="sm" onClick={detect}><ShieldAlert className="h-4 w-4" /> Detect Conflicts</Button>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="sm" disabled={view.stats.class_count === 0}>
                <Download className="h-4 w-4" /> Export <ChevronDown className="h-4 w-4 text-muted-foreground" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent>
              {EXPORT_FORMATS.map((f) => (
                <DropdownMenuItem key={f.fmt} onSelect={() => download(f.fmt, f.ext)}>
                  {f.label}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
          {canWrite && (
            <Button size="sm" onClick={() => setExtractOpen(true)}><Sparkles className="h-4 w-4" /> Extract from Documents</Button>
          )}
        </div>
      </div>
      )}

      {activeJob && (activeJob.status === "running" || activeJob.status === "pending") && (
        <div className="flex items-center gap-3 rounded-lg border border-primary/30 bg-primary/5 px-4 py-2.5 text-sm">
          <Loader2 className="h-4 w-4 animate-spin text-primary" />
          <span className="font-medium">Extracting…</span>
          <span className="text-muted-foreground">
            {activeJob.processed_chunks}/{activeJob.total_chunks} chunks · {activeJob.model}
          </span>
          <div className="ml-auto h-1.5 w-40 overflow-hidden rounded-full bg-muted">
            <div
              className="h-full bg-primary transition-all"
              style={{ width: `${activeJob.total_chunks ? Math.max(6, (activeJob.processed_chunks / activeJob.total_chunks) * 100) : 6}%` }}
            />
          </div>
        </div>
      )}

      {section === "overview" && (
        <KsOverview ks={ks} view={view} sources={sources} conflicts={conflicts} />
      )}

      {/* Graph + Classes/Properties/Axioms are one workbench. Full-bleed: negative margins cancel
          <main>'s padding so it sits flush against the sidebar / top bar / viewport edges. */}
      {section === "ontology" && (
        <div className="-m-4 md:-m-6">
          <OntologyWorkbench
            key={`${searchParams.get("view") ?? "graph"}-${searchParams.get("tab") ?? ""}`}
            view={view}
            canWrite={canWrite}
            initialLens={searchParams.get("view") === "table" ? "table" : "graph"}
            initialTab={searchParams.get("tab") === "axioms" ? "axioms" : "classes"}
            axioms={axiomGroups}
            onAddAxiom={() => setAxiomOpen(true)}
            onAddClass={() => setClassDialog({ open: true, initial: null })}
            onEditClass={(c) => setClassDialog({ open: true, initial: c })}
            onDeleteClass={async (c) => {
              let n = 0
              try { n = (await api.aboxClasses(ksId)).classes.find((x) => x.iri === c.iri)?.count ?? 0 } catch { /* count is best-effort */ }
              const msg = n > 0
                ? `Delete class "${c.label}"? It has ${n} instance${n === 1 ? "" : "s"} — they'll be deleted too (an instance kept only if it also has another type).`
                : `Delete class "${c.label}" and its relations?`
              if (confirm(msg)) runEdit({ op: "delete_class", iri: c.iri }, "Deleted")
            }}
            onAddProperty={() => setPropDialog({ open: true, initial: null })}
            onEditProperty={(p, kind) => setPropDialog({ open: true, initial: { ...p, kind } })}
            onDeleteProperty={(p) => confirm(`Delete property "${p.label}"? Instance assertions that use it will be deleted too.`) && runEdit({ op: "delete_property", iri: p.iri }, "Deleted")}
          />
        </div>
      )}

      {section === "instances" && (
        <InstancesPanel ksId={ksId} view={view} canWrite={canWrite} onChanged={refresh} />
      )}

      {/* Review is a second-level menu; the active sub-page (conflicts / resolution / validation /
          learned memory) is chosen by the sidebar and reflected in the URL. */}
      {section === "review" && (
        <ReviewPanel ksId={ksId} sub={reviewSub} canWrite={canWrite} onChanged={refresh} />
      )}

      {section === "documents" && (
        <KsDocuments ksId={ksId} canWrite={canWrite} onChanged={refresh} />
      )}

      {section === "history" && <KsHistory ksId={ksId} canWrite={canWrite} onChanged={refresh} />}

      {section === "members" && (
        <MembersPanel ksId={ksId} canManage={canManage} />
      )}

      <ExtractDialog
        ksId={ksId} open={extractOpen} onOpenChange={setExtractOpen}
        mode="both" selectableModes={["both", "tbox"]}
        onStarted={(job) => { setActiveJob(job); refresh() }}
      />
      <ClassDialog
        ksId={ksId} open={classDialog.open} initial={classDialog.initial}
        onOpenChange={(o) => setClassDialog((s) => ({ ...s, open: o }))} onSaved={applyResult}
      />
      <PropertyDialog
        ksId={ksId} open={propDialog.open} initial={propDialog.initial} classes={view.classes}
        onOpenChange={(o) => setPropDialog((s) => ({ ...s, open: o }))} onSaved={applyResult}
      />
      <AxiomDialog
        ksId={ksId} open={axiomOpen} classes={view.classes}
        onOpenChange={setAxiomOpen} onSaved={applyResult}
      />
    </div>
  )
}
