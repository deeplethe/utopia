import { useCallback, useEffect, useRef, useState } from "react"
import { Navigate, useParams, useSearchParams } from "react-router-dom"
import { toast } from "sonner"
import { ChevronDown, Crown, Download, Eye, FileUp, Loader2, RefreshCw, Shield, ShieldAlert, Sparkles } from "lucide-react"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type { Conflict, EditOp, EditResult, ExtractionJob, KnowledgeSystem, OntologyProperty, OntologyView, Role, SourceDoc } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import OntologyWorkbench from "@/components/OntologyWorkbench"
import ExtractDialog from "@/components/ExtractDialog"
import InstancesPanel from "@/components/InstancesPanel"
import ReviewPanel from "@/components/ReviewPanel"
import DocumentsExtractionPanel from "@/components/DocumentsExtractionPanel"
import KsHistory from "@/components/KsHistory"
import KsOverview from "@/components/KsOverview"
import MembersPanel from "@/components/MembersPanel"
import ApiAccessPanel from "@/components/ApiAccessPanel"
import RdfImportDialog from "@/components/RdfImportDialog"
import VocabularyPanel from "@/components/VocabularyPanel"
import PromptSettingsPanel from "@/components/PromptSettingsPanel"
import ReleasePanel from "@/components/ReleasePanel"
import { AxiomDialog, ClassDialog, PropertyDialog } from "@/components/EditDialogs"

function RoleTag({ role }: { role: Role }) {
  const { t } = useI18n()
  if (role === "owner") return <Badge variant="outline" className="gap-1"><Crown className="h-3 w-3" /> {t("common.owner")}</Badge>
  if (role === "editor") return <Badge variant="outline" className="gap-1"><Shield className="h-3 w-3" /> {t("common.editor")}</Badge>
  return <Badge variant="outline" className="gap-1"><Eye className="h-3 w-3" /> {t("common.viewer")}</Badge>
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
  const { locale, t } = useI18n()
  const confirmAction = useConfirm()
  const { id, section: sectionParam, sub: subParam } = useParams()
  const [searchParams] = useSearchParams()
  const ksId = Number(id)
  const section = sectionParam ?? "overview"
  const REVIEW_SUBS = ["conflicts", "resolution", "terminology", "validation"]
  const reviewSub = subParam && REVIEW_SUBS.includes(subParam) ? subParam : "conflicts"
  const [ks, setKs] = useState<KnowledgeSystem | null>(null)
  const [view, setView] = useState<OntologyView | null>(null)
  const [jobs, setJobs] = useState<ExtractionJob[]>([])
  const [sources, setSources] = useState<SourceDoc[]>([])
  const [conflicts, setConflicts] = useState<Conflict[]>([])
  const [loading, setLoading] = useState(true)
  const [extractOpen, setExtractOpen] = useState(false)
  const [rdfImportOpen, setRdfImportOpen] = useState(false)
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
      toast.error(t("common.failedLoad", { error: (e as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [ksId, t])

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
        toast.success(activeJob.kind === "both"
          ? t("ontology.extractionCompleteBoth", {
              classes: activeJob.classes_added,
              properties: activeJob.properties_added,
              individuals: activeJob.individuals_added,
              queued: activeJob.pending_added,
              terms: activeJob.terms_added,
              proposals: activeJob.terminology_proposals,
            })
          : t("ontology.extractionCompleteTbox", {
              classes: activeJob.classes_added,
              properties: activeJob.properties_added,
              axioms: activeJob.axioms_added,
              terms: activeJob.terms_added,
              proposals: activeJob.terminology_proposals,
            }))
      else toast.error(t("ontology.extractionFailed", { error: activeJob.error ?? t("ontology.unknownError") }))
      if (activeJob.terminology_error) {
        toast.warning(t("ontology.terminologyFailed", { error: activeJob.terminology_error }))
      }
      window.dispatchEvent(new Event("ontopilot:vocabulary-changed"))
      window.dispatchEvent(new Event("ontopilot:review-counts-changed"))
      setActiveJob(null)
      refresh()
      return
    }
    const timer = setTimeout(async () => {
      try {
        setActiveJob(await api.getJob(ksId, activeJob.id))
      } catch {
        setActiveJob(null)
      }
    }, 1500)
    return () => clearTimeout(timer)
  }, [activeJob, ksId, refresh, t])

  const applyResult = (res: EditResult) => {
    setView(res.view)
    setConflicts(res.open_conflicts)
  }

  const runEdit = useCallback(async (op: EditOp, successMsg?: string) => {
    try {
      applyResult(await api.editOntology(ksId, op))
      if (successMsg) toast.success(successMsg)
    } catch (e) {
      toast.error(t("ontology.operationFailed", { error: (e as Error).message }))
    }
  }, [ksId, t])

  const detect = useCallback(async () => {
    try {
      const c = await api.detectConflicts(ksId)
      setConflicts(c)
      toast[c.length ? "warning" : "success"](c.length
        ? t("ontology.foundConflicts", { count: c.length })
        : t("ontology.noConflicts"))
    } catch (e) {
      toast.error(t("ontology.detectionFailed", { error: (e as Error).message }))
    }
  }, [ksId, t])

  const download = useCallback(async (fmt: string, ext: string) => {
    try {
      const data = await api.exportOntology(ksId, fmt)
      const url = URL.createObjectURL(new Blob([data], { type: "application/octet-stream" }))
      const a = document.createElement("a")
      a.href = url; a.download = `${ks?.name ?? "ontology"}.${ext}`; a.click()
      URL.revokeObjectURL(url)
    } catch (e) { toast.error(t("ontology.exportFailed", { error: (e as Error).message })) }
  }, [ksId, ks, t])

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
  if (["conflicts", "resolution", "terminology", "validation"].includes(section))
    return <Navigate to={`/knowledge/${ksId}/review/${section}`} replace />
  if (section === "review" && !subParam) return <Navigate to={`/knowledge/${ksId}/review/conflicts`} replace />
  if (loading) return <p className="text-sm text-muted-foreground">{t("common.loading")}</p>
  if (!ks || !view) return <p className="text-sm text-muted-foreground">{t("ontology.notFound")}</p>

  const canWrite = ks.my_role === "editor" || ks.my_role === "owner"
  const canManage = ks.my_role === "owner"

  const axiomGroups = [
    {
      type: t("overview.subclass"),
      title: t("ontology.subclassAxioms", { count: view.axioms.subclass_of.length }),
      items: view.axioms.subclass_of.map((r) => ({
        text: `${label(r.sub)} ⊑ ${label(r.super)}`,
        parts: { left: label(r.sub), op: "sub" as const, right: label(r.super) },
        onDelete: canWrite ? () => runEdit({ op: "delete_axiom", type: "subclass", sub: r.sub, super: r.super }, t("common.deleted")) : undefined,
      })),
    },
    {
      type: t("overview.disjoint"),
      title: t("ontology.disjointAxioms", { count: view.axioms.disjoint_with.length }),
      items: view.axioms.disjoint_with.map((r) => ({
        text: `${label(r.a)} ⟂ ${label(r.b)}`,
        parts: { left: label(r.a), op: "disjoint" as const, right: label(r.b) },
        onDelete: canWrite ? () => runEdit({ op: "delete_axiom", type: "disjoint", a: r.a, b: r.b }, t("common.deleted")) : undefined,
      })),
    },
    {
      type: t("overview.equivalent"),
      title: t("ontology.equivalentAxioms", { count: view.axioms.equivalent_class.length }),
      items: view.axioms.equivalent_class.map((r) => ({
        text: `${label(r.a)} ≡ ${label(r.b)}`,
        parts: { left: label(r.a), op: "equiv" as const, right: label(r.b) },
        onDelete: canWrite ? () => runEdit({ op: "delete_axiom", type: "equivalent", a: r.a, b: r.b }, t("common.deleted")) : undefined,
      })),
    },
  ]

  return (
    <div className="space-y-5">
      {section === "overview" && (
      <div className="flex flex-wrap items-start justify-between gap-4 border-b pb-5">
        <div className="min-w-0 max-w-3xl space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="truncate text-xl font-semibold tracking-tight">{ks.name}</h1>
            <RoleTag role={ks.my_role} />
          </div>
          <p className="text-sm leading-relaxed text-muted-foreground">
            {ks.description || t("common.noDescription")}
          </p>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
            <span>{t("overview.updated")} {new Date(ks.updated_at).toLocaleString(locale)}</span>
            <code className="max-w-full truncate rounded bg-muted px-1.5 py-0.5" title={ks.base_iri}>{ks.base_iri}</code>
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" size="sm" onClick={refresh}><RefreshCw className="h-4 w-4" /> {t("common.refresh")}</Button>
          {canWrite && (
            <Button variant="outline" size="sm" onClick={detect}><ShieldAlert className="h-4 w-4" /> {t("ontology.detectConflicts")}</Button>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="sm" disabled={view.stats.class_count === 0}>
                <Download className="h-4 w-4" /> {t("ontology.export")} <ChevronDown className="h-4 w-4 text-muted-foreground" />
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
            <Button variant="outline" size="sm" onClick={() => setRdfImportOpen(true)}><FileUp className="h-4 w-4" /> {t("ontology.importRdf")}</Button>
          )}
          {canWrite && (
            <Button size="sm" onClick={() => setExtractOpen(true)}><Sparkles className="h-4 w-4" /> {t("ontology.extractDocuments")}</Button>
          )}
        </div>
      </div>
      )}

      {activeJob && section !== "documents" && (activeJob.status === "running" || activeJob.status === "pending") && (
        <div className="flex items-center gap-3 rounded-lg border border-primary/30 bg-primary/5 px-4 py-2.5 text-sm">
          <Loader2 className="h-4 w-4 animate-spin text-primary" />
          <span className="font-medium">
            {activeJob.phase ? t(`extract.phase.${activeJob.phase}`) : t("ontology.extracting")}
          </span>
          <span className="text-muted-foreground">
            {activeJob.processed_chunks}/{activeJob.total_chunks} {t("ontology.chunks")} · {activeJob.model}
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
        <KsOverview ks={ks} view={view} sources={sources} conflicts={conflicts} jobs={jobs} />
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
                ? t("ontology.deleteClassInstances", { name: c.label, count: n })
                : t("ontology.deleteClass", { name: c.label })
              if (await confirmAction(msg, { destructive: true })) runEdit({ op: "delete_class", iri: c.iri }, t("common.deleted"))
            }}
            onAddProperty={() => setPropDialog({ open: true, initial: null })}
            onEditProperty={(p, kind) => setPropDialog({ open: true, initial: { ...p, kind } })}
            onDeleteProperty={async (p) => {
              if (await confirmAction(t("ontology.deleteProperty", { name: p.label }), { destructive: true })) {
                runEdit({ op: "delete_property", iri: p.iri }, t("common.deleted"))
              }
            }}
          />
        </div>
      )}

      {section === "instances" && (
        <InstancesPanel ksId={ksId} view={view} canWrite={canWrite} onChanged={refresh} />
      )}

      {section === "vocabulary" && (
        <VocabularyPanel ksId={ksId} view={view} canWrite={canWrite} />
      )}

      {/* Review is a second-level menu; the active sub-page (conflicts / resolution / validation /
          learned memory) is chosen by the sidebar and reflected in the URL. */}
      {section === "review" && (
        <ReviewPanel ksId={ksId} sub={reviewSub} view={view} canWrite={canWrite} onChanged={refresh} />
      )}

      {section === "documents" && (
        <DocumentsExtractionPanel ksId={ksId} canWrite={canWrite} onChanged={refresh} />
      )}

      {section === "prompts" && (
        <PromptSettingsPanel ksId={ksId} canWrite={canWrite} />
      )}

      {section === "releases" && (
        <ReleasePanel ksId={ksId} canWrite={canWrite} canManage={canManage} onChanged={refresh} />
      )}

      {section === "history" && <KsHistory ksId={ksId} canWrite={canWrite} onChanged={refresh} />}

      {section === "members" && (
        <MembersPanel ksId={ksId} canManage={canManage} />
      )}

      {section === "api" && (
        <ApiAccessPanel ks={ks} canManage={canManage} />
      )}

      <ExtractDialog
        ksId={ksId} open={extractOpen} onOpenChange={setExtractOpen}
        mode="both" selectableModes={["both", "tbox"]}
        onStarted={(job) => { setActiveJob(job); refresh() }}
      />
      <RdfImportDialog
        ksId={ksId}
        baseIri={ks.base_iri}
        open={rdfImportOpen}
        onOpenChange={setRdfImportOpen}
        onImported={(result) => {
          setView(result.view)
          setConflicts(result.open_conflicts)
          refresh()
        }}
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
