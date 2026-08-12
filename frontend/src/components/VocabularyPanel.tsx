import { useCallback, useEffect, useMemo, useState } from "react"
import {
  ChevronLeft, ChevronRight, Download, Link2, Loader2, Pencil, Plus, RefreshCw, RotateCcw,
  Search, Tags, Trash2,
} from "lucide-react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type {
  OntologyView, VocabularyConcept, VocabularyConceptInput, VocabularyScheme, VocabularyView,
} from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Combobox, type ComboboxOption } from "@/components/ui/combobox"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { Textarea } from "@/components/ui/textarea"

const emptyVocabulary: VocabularyView = {
  schemes: [], concepts: [], stats: {
    scheme_count: 0, concept_count: 0, label_count: 0, mapped_count: 0, unmapped_count: 0,
  },
}
const PAGE_SIZE = 20
type StatusFilter = "all" | "active" | "deprecated"
type MappingFilter = "all" | "mapped" | "standalone"
type OriginFilter = "all" | "manual" | "extraction" | "agent"

function splitLabels(value: string): string[] {
  return [...new Set(value.split(/[,，\n]+/).map((item) => item.trim()).filter(Boolean))]
}

function pickPrimaryScheme(schemes: VocabularyScheme[]): string {
  const fixed = schemes.find((scheme) => scheme.iri.endsWith("#scheme-extracted"))
  if (fixed) return fixed.iri
  if (schemes.length === 1) return schemes[0].iri
  const generated = schemes.filter((scheme) => scheme.origin === "extraction")
  const candidates = generated.length > 0 ? generated : schemes
  return [...candidates].sort((left, right) => right.concept_count - left.concept_count)[0]?.iri ?? ""
}

export default function VocabularyPanel({
  ksId, view, canWrite,
}: {
  ksId: number
  view: OntologyView
  canWrite: boolean
}) {
  const { t } = useI18n()
  const confirmAction = useConfirm()
  const [data, setData] = useState<VocabularyView>(emptyVocabulary)
  const [selectedSchemeIri, setSelectedSchemeIri] = useState("")
  const [query, setQuery] = useState("")
  const [debouncedQuery, setDebouncedQuery] = useState("")
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all")
  const [mappingFilter, setMappingFilter] = useState<MappingFilter>("all")
  const [originFilter, setOriginFilter] = useState<OriginFilter>("all")
  const [startDate, setStartDate] = useState("")
  const [endDate, setEndDate] = useState("")
  const [page, setPage] = useState(0)
  const [conceptTotal, setConceptTotal] = useState(0)
  const [loading, setLoading] = useState(true)
  const [conceptLoading, setConceptLoading] = useState(false)
  const [schemeDialog, setSchemeDialog] = useState<{ open: boolean; initial: VocabularyScheme | null }>({ open: false, initial: null })
  const [termDialog, setTermDialog] = useState<{ open: boolean; initial: VocabularyConcept | null }>({ open: false, initial: null })
  const [labelDetails, setLabelDetails] = useState<VocabularyConcept | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const next = await api.listVocabularySchemes(ksId)
      setData((current) => ({ ...current, schemes: next.items, stats: next.stats }))
      setSelectedSchemeIri((current) => {
        if (current && next.items.some((scheme) => scheme.iri === current)) return current
        return pickPrimaryScheme(next.items)
      })
    } catch (error) {
      toast.error(t("common.failedLoad", { error: (error as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [ksId, t])

  useEffect(() => { load() }, [load])

  useEffect(() => {
    const timer = setTimeout(() => setDebouncedQuery(query), 300)
    return () => clearTimeout(timer)
  }, [query])
  useEffect(() => { setPage(0) }, [
    debouncedQuery, selectedSchemeIri, statusFilter, mappingFilter, originFilter, startDate, endDate,
  ])
  useEffect(() => {
    setPage((current) => Math.min(
      current,
      Math.max(0, Math.ceil(conceptTotal / PAGE_SIZE) - 1),
    ))
  }, [conceptTotal])

  const loadConcepts = useCallback(async () => {
    if (!selectedSchemeIri) {
      setData((current) => ({ ...current, concepts: [] }))
      setConceptTotal(0)
      return
    }
    setConceptLoading(true)
    try {
      const next = await api.listVocabularyConcepts(ksId, {
        scheme_iri: selectedSchemeIri,
        q: debouncedQuery || undefined,
        status: statusFilter === "all" ? undefined : statusFilter,
        mapping: mappingFilter === "all" ? undefined : mappingFilter,
        origin: originFilter === "all" ? undefined : originFilter,
        start_date: startDate || undefined,
        end_date: endDate || undefined,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      })
      setData((current) => ({ ...current, concepts: next.items }))
      setConceptTotal(next.total)
    } catch (error) {
      toast.error(t("common.failedLoad", { error: (error as Error).message }))
    } finally {
      setConceptLoading(false)
    }
  }, [
    debouncedQuery, endDate, ksId, mappingFilter, originFilter, page, selectedSchemeIri,
    startDate, statusFilter, t,
  ])

  useEffect(() => { void loadConcepts() }, [loadConcepts])

  const refresh = useCallback(async () => {
    await load()
    await loadConcepts()
  }, [load, loadConcepts])

  useEffect(() => {
    const reload = () => { void refresh() }
    window.addEventListener("ontopilot:vocabulary-changed", reload)
    return () => window.removeEventListener("ontopilot:vocabulary-changed", reload)
  }, [refresh])

  const selectedScheme = data.schemes.find((scheme) => scheme.iri === selectedSchemeIri) ?? null
  const primarySchemeIri = pickPrimaryScheme(data.schemes)
  const showSchemeSwitcher = data.schemes.length > 1
  const entityLabels = useMemo(() => Object.fromEntries([
    ...view.classes.map((entity) => [entity.iri, entity.label]),
    ...view.object_properties.map((entity) => [entity.iri, entity.label]),
    ...view.data_properties.map((entity) => [entity.iri, entity.label]),
  ]), [view])
  const entityOptions = useMemo<ComboboxOption[]>(() => [
    { value: "", label: t("vocabulary.noMapping") },
    ...view.classes.map((entity) => ({ value: entity.iri, label: entity.label, hint: t("common.class") })),
    ...view.object_properties.map((entity) => ({ value: entity.iri, label: entity.label, hint: t("common.properties") })),
    ...view.data_properties.map((entity) => ({ value: entity.iri, label: entity.label, hint: t("common.properties") })),
  ], [t, view])

  const shownConcepts = data.concepts
  const pageCount = Math.max(1, Math.ceil(conceptTotal / PAGE_SIZE))
  const hasFilters = Boolean(
    query || statusFilter !== "all" || mappingFilter !== "all" || originFilter !== "all"
    || startDate || endDate,
  )

  const resetFilters = () => {
    setQuery("")
    setDebouncedQuery("")
    setStatusFilter("all")
    setMappingFilter("all")
    setOriginFilter("all")
    setStartDate("")
    setEndDate("")
    setPage(0)
  }

  const exportSkos = async () => {
    try {
      const content = await api.exportVocabulary(ksId)
      const url = URL.createObjectURL(new Blob([content], { type: "text/turtle" }))
      const anchor = document.createElement("a")
      anchor.href = url
      anchor.download = `${selectedScheme?.title ?? "vocabulary"}.ttl`
      anchor.click()
      URL.revokeObjectURL(url)
    } catch (error) {
      toast.error(t("review.failed", { error: (error as Error).message }))
    }
  }

  const removeScheme = async (scheme: VocabularyScheme) => {
    if (!await confirmAction(t("vocabulary.deleteSchemeConfirm", { name: scheme.title, count: scheme.concept_count }), { destructive: true })) return
    try {
      await api.deleteVocabularyScheme(ksId, scheme.iri)
      toast.success(t("common.deleted"))
      await refresh()
    } catch (error) {
      toast.error(t("common.failedDelete", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    }
  }

  const removeConcept = async (concept: VocabularyConcept) => {
    if (!await confirmAction(t("vocabulary.deleteTermConfirm", { name: concept.display_label }), { destructive: true })) return
    try {
      await api.deleteVocabularyConcept(ksId, concept.iri)
      toast.success(t("common.deleted"))
      await refresh()
    } catch (error) {
      toast.error(t("common.failedDelete", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    }
  }

  if (loading) return <p className="text-sm text-muted-foreground">{t("common.loading")}</p>

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">{t("vocabulary.title")}</h1>
          <div className="mt-2 flex flex-wrap gap-2">
            <Badge variant="secondary">{data.stats.concept_count} {t("vocabulary.terms")}</Badge>
            <Badge variant="secondary">{data.stats.mapped_count} {t("vocabulary.mapped")}</Badge>
            <Badge variant="secondary">{data.stats.unmapped_count} {t("vocabulary.standalone")}</Badge>
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" size="sm" onClick={exportSkos} disabled={data.stats.concept_count === 0}>
            <Download className="h-4 w-4" /> {t("vocabulary.export")}
          </Button>
          {canWrite && (
            <Button size="sm" onClick={() => setTermDialog({ open: true, initial: null })} disabled={!selectedSchemeIri}>
              <Plus className="h-4 w-4" /> {t("vocabulary.newTerm")}
            </Button>
          )}
        </div>
      </div>

      {data.schemes.length === 0 ? (
        <div className="rounded-lg border px-6 py-16 text-center">
          <Tags className="mx-auto mb-3 h-8 w-8 text-muted-foreground" />
          <h2 className="font-medium">{t("vocabulary.noSchemes")}</h2>
          <p className="mx-auto mt-1 max-w-lg text-sm text-muted-foreground">{t("vocabulary.noSchemesDescription")}</p>
        </div>
      ) : (
        <div className={showSchemeSwitcher ? "grid gap-4 lg:grid-cols-[15rem_minmax(0,1fr)]" : ""}>
          {showSchemeSwitcher && <aside className="h-fit rounded-lg border">
            <div className="border-b px-3 py-2.5">
              <span className="text-sm font-medium">{t("vocabulary.schemes")}</span>
            </div>
            <div className="space-y-1 p-2">
              {data.schemes.map((scheme) => (
                <div key={scheme.iri} className={`group flex items-center rounded-md ${selectedSchemeIri === scheme.iri ? "bg-muted" : "hover:bg-muted/60"}`}>
                  <button type="button" className="min-w-0 flex-1 px-2 py-2 text-left" onClick={() => setSelectedSchemeIri(scheme.iri)}>
                    <span className="flex items-center gap-1.5 truncate text-sm font-medium">
                      <span className="truncate">{scheme.title}</span>
                      {scheme.origin === "extraction" && <Badge variant="outline" className="h-4 px-1 text-[9px] font-normal">{t("vocabulary.origin.extraction")}</Badge>}
                    </span>
                    <span className="text-xs text-muted-foreground">{scheme.concept_count} {t("vocabulary.terms")} · {scheme.default_language}</span>
                  </button>
                  {canWrite && (
                    <div className="mr-1 flex opacity-0 transition-opacity group-hover:opacity-100">
                      <Button size="icon-xs" variant="ghost" title={t("common.edit")}
                        onClick={() => setSchemeDialog({ open: true, initial: scheme })}>
                        <Pencil className="h-3 w-3" />
                      </Button>
                      <Button size="icon-xs" variant="ghost" className="text-muted-foreground hover:text-destructive"
                        title={t("common.delete")} onClick={() => removeScheme(scheme)}>
                        <Trash2 className="h-3 w-3" />
                      </Button>
                    </div>
                  )}
                </div>
              ))}
            </div>
          </aside>}

          <section className="min-w-0 space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                <h2 className="text-sm font-semibold">{selectedScheme?.title}</h2>
                <Badge variant="outline" className="h-5 text-[10px] tabular-nums">{conceptTotal}</Badge>
                {selectedSchemeIri === primarySchemeIri && <Badge variant="secondary" className="h-5 text-[10px]">{t("vocabulary.primary")}</Badge>}
                {canWrite && !showSchemeSwitcher && selectedScheme && (
                  <Button size="icon-xs" variant="ghost" title={t("common.edit")}
                    onClick={() => setSchemeDialog({ open: true, initial: selectedScheme })}>
                    <Pencil className="h-3 w-3" />
                  </Button>
                )}
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-2 rounded-lg border bg-muted/20 p-2">
              <div className="relative min-w-48 flex-1 sm:max-w-64">
                <Search className="absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input value={query} onChange={(event) => setQuery(event.target.value)}
                  placeholder={t("vocabulary.searchTerms")} className="h-8 pl-8 text-sm" />
              </div>
              <Select value={statusFilter} onValueChange={(value) => setStatusFilter(value as StatusFilter)}>
                <SelectTrigger className="h-8 w-32 text-sm"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">{t("vocabulary.statusAll")}</SelectItem>
                  <SelectItem value="active">{t("vocabulary.status.active")}</SelectItem>
                  <SelectItem value="deprecated">{t("vocabulary.status.deprecated")}</SelectItem>
                </SelectContent>
              </Select>
              <Select value={mappingFilter} onValueChange={(value) => setMappingFilter(value as MappingFilter)}>
                <SelectTrigger className="h-8 w-36 text-sm"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">{t("vocabulary.mappingAll")}</SelectItem>
                  <SelectItem value="mapped">{t("vocabulary.mapped")}</SelectItem>
                  <SelectItem value="standalone">{t("vocabulary.standalone")}</SelectItem>
                </SelectContent>
              </Select>
              <Select value={originFilter} onValueChange={(value) => setOriginFilter(value as OriginFilter)}>
                <SelectTrigger className="h-8 w-36 text-sm"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">{t("vocabulary.originAll")}</SelectItem>
                  <SelectItem value="extraction">{t("vocabulary.origin.extraction")}</SelectItem>
                  <SelectItem value="manual">{t("vocabulary.origin.manual")}</SelectItem>
                  <SelectItem value="agent">{t("vocabulary.origin.agent")}</SelectItem>
                </SelectContent>
              </Select>
              <div className="flex items-center gap-1" title={t("vocabulary.updatedRange")}>
                <span className="mr-1 whitespace-nowrap text-xs text-muted-foreground">{t("vocabulary.updatedRange")}</span>
                <Input
                  type="date"
                  value={startDate}
                  max={endDate || undefined}
                  onChange={(event) => setStartDate(event.target.value)}
                  aria-label={t("vocabulary.startDate")}
                  title={t("vocabulary.startDate")}
                  className="h-8 w-[8.8rem] text-sm"
                />
                <span className="text-xs text-muted-foreground">–</span>
                <Input
                  type="date"
                  value={endDate}
                  min={startDate || undefined}
                  onChange={(event) => setEndDate(event.target.value)}
                  aria-label={t("vocabulary.endDate")}
                  title={t("vocabulary.endDate")}
                  className="h-8 w-[8.8rem] text-sm"
                />
              </div>
              <Button size="sm" variant="outline" className="h-8 gap-1.5" onClick={resetFilters} disabled={!hasFilters}>
                <RotateCcw className="h-3.5 w-3.5" /> {t("common.reset")}
              </Button>
              <Button size="icon" variant="outline" className="h-8 w-8" onClick={() => { void refresh() }}
                disabled={loading || conceptLoading} title={t("common.refresh")}>
                <RefreshCw className={`h-3.5 w-3.5 ${loading || conceptLoading ? "animate-spin" : ""}`} />
              </Button>
            </div>
            <div className="rounded-lg border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>{t("vocabulary.preferredTerm")}</TableHead>
                    <TableHead>{t("vocabulary.alternativeLabels")}</TableHead>
                    <TableHead>{t("vocabulary.broader")}</TableHead>
                    <TableHead>{t("vocabulary.mapping")}</TableHead>
                    <TableHead className="w-24">{t("common.status")}</TableHead>
                    <TableHead className="w-24 text-right">{t("common.actions")}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {conceptLoading ? (
                    <TableRow><TableCell colSpan={6} className="h-24 text-center text-muted-foreground">{t("common.loading")}</TableCell></TableRow>
                  ) : shownConcepts.length === 0 ? (
                    <TableRow><TableCell colSpan={6} className="h-24 text-center text-muted-foreground">{t("vocabulary.noTerms")}</TableCell></TableRow>
                  ) : shownConcepts.map((concept) => (
                    <TableRow key={concept.iri}>
                      <TableCell>
                        <div className="font-medium">{concept.display_label}</div>
                      </TableCell>
                      <TableCell className="max-w-72 text-sm text-muted-foreground">
                        {concept.alt_labels.length === 0 ? "—" : (
                          <div className="flex min-w-0 flex-wrap gap-1">
                            {concept.alt_labels.slice(0, 2).map((label) => (
                              <Badge key={`${label.language}:${label.value}`} variant="secondary" className="max-w-36 font-normal">
                                <span className="truncate" title={label.value}>{label.value}</span>
                              </Badge>
                            ))}
                            {concept.alt_labels.length > 2 && (
                              <button type="button"
                                className="rounded-full border px-2 py-0.5 text-[11px] font-medium text-foreground hover:bg-muted"
                                title={t("vocabulary.viewAllLabels", { count: concept.alt_labels.length })}
                                onClick={() => setLabelDetails(concept)}>
                                +{concept.alt_labels.length - 2}
                              </button>
                            )}
                          </div>
                        )}
                      </TableCell>
                      <TableCell className="text-sm text-muted-foreground">{concept.broader_labels.join("、") || "—"}</TableCell>
                      <TableCell className="max-w-52">
                        {concept.mapped_entity_iri ? (
                          <span className="flex items-center gap-1.5 truncate text-sm" title={concept.mapped_entity_iri}>
                            <Link2 className="h-3.5 w-3.5 shrink-0 text-primary" />
                            {entityLabels[concept.mapped_entity_iri] ?? concept.mapped_entity_iri.split(/[#/]/).pop()}
                          </span>
                        ) : <span className="text-sm text-muted-foreground" title={t("vocabulary.standaloneHint")}>{t("vocabulary.standalone")}</span>}
                      </TableCell>
                      <TableCell>
                        <Badge variant="outline" className={concept.status === "deprecated" ? "text-muted-foreground" : "text-emerald-600"}>
                          {t(concept.status === "active" ? "vocabulary.status.active" : "vocabulary.status.deprecated")}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-right">
                        {canWrite && (
                          <div className="flex justify-end gap-1">
                            <Button size="icon-sm" variant="ghost" title={t("common.edit")}
                              onClick={() => setTermDialog({ open: true, initial: concept })}>
                              <Pencil className="h-3.5 w-3.5" />
                            </Button>
                            <Button size="icon-sm" variant="ghost" className="text-muted-foreground hover:text-destructive"
                              title={t("common.delete")} onClick={() => removeConcept(concept)}>
                              <Trash2 className="h-3.5 w-3.5" />
                            </Button>
                          </div>
                        )}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
            {conceptTotal > PAGE_SIZE && (
              <div className="flex items-center justify-between text-xs text-muted-foreground">
                <span>{t("review.page", {
                  start: page * PAGE_SIZE + 1,
                  end: Math.min(conceptTotal, (page + 1) * PAGE_SIZE),
                  total: conceptTotal,
                })}</span>
                <div className="flex gap-1">
                  <Button size="icon" variant="outline" className="h-7 w-7" disabled={conceptLoading || page === 0}
                    onClick={() => setPage((current) => current - 1)}>
                    <ChevronLeft className="h-4 w-4" />
                  </Button>
                  <Button size="icon" variant="outline" className="h-7 w-7"
                    disabled={conceptLoading || page >= pageCount - 1}
                    onClick={() => setPage((current) => current + 1)}>
                    <ChevronRight className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            )}
          </section>
        </div>
      )}

      <SchemeDialog
        ksId={ksId} state={schemeDialog} onOpenChange={(open) => setSchemeDialog((current) => ({ ...current, open }))}
        onSaved={async () => { setSchemeDialog({ open: false, initial: null }); await refresh() }}
      />
      <TermDialog
        ksId={ksId} state={termDialog} schemes={data.schemes}
        selectedSchemeIri={selectedSchemeIri} entityOptions={entityOptions}
        onOpenChange={(open) => setTermDialog((current) => ({ ...current, open }))}
        onSaved={async () => { setTermDialog({ open: false, initial: null }); await refresh() }}
      />
      <Dialog open={labelDetails != null} onOpenChange={(open) => { if (!open) setLabelDetails(null) }}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>{t("vocabulary.allLabels")}</DialogTitle>
            <DialogDescription>{labelDetails?.display_label}</DialogDescription>
          </DialogHeader>
          {labelDetails && (
            <div className="max-h-[60vh] space-y-5 overflow-y-auto">
              <LabelGroup title={t("vocabulary.alternativeLabels")} labels={labelDetails.alt_labels} />
              {labelDetails.hidden_labels.length > 0 && (
                <LabelGroup title={t("vocabulary.hiddenLabels")} labels={labelDetails.hidden_labels} />
              )}
            </div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  )
}

function LabelGroup({ title, labels }: { title: string; labels: VocabularyConcept["alt_labels"] }) {
  return (
    <section className="space-y-2">
      <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{title}</h3>
      <div className="space-y-1.5">
        {labels.map((label) => (
          <div key={`${label.language}:${label.value}`} className="flex items-start justify-between gap-3 rounded-md border px-3 py-2 text-sm">
            <span className="min-w-0 break-words">{label.value}</span>
            <Badge variant="outline" className="shrink-0 font-normal">{label.language}</Badge>
          </div>
        ))}
      </div>
    </section>
  )
}

function SchemeDialog({
  ksId, state, onOpenChange, onSaved,
}: {
  ksId: number
  state: { open: boolean; initial: VocabularyScheme | null }
  onOpenChange: (open: boolean) => void
  onSaved: () => void
}) {
  const { t } = useI18n()
  const [title, setTitle] = useState("")
  const [description, setDescription] = useState("")
  const [language, setLanguage] = useState("zh-CN")
  const [saving, setSaving] = useState(false)
  useEffect(() => {
    if (!state.open) return
    setTitle(state.initial?.title ?? "")
    setDescription(state.initial?.description ?? "")
    setLanguage(state.initial?.default_language ?? "zh-CN")
  }, [state])

  const save = async () => {
    if (!title.trim()) return
    setSaving(true)
    try {
      const body = { title: title.trim(), description: description.trim(), default_language: language.trim() || "zh-CN" }
      if (state.initial) await api.updateVocabularyScheme(ksId, state.initial.iri, body)
      else await api.createVocabularyScheme(ksId, body)
      toast.success(t(state.initial ? "vocabulary.schemeUpdated" : "vocabulary.schemeCreated"))
      onSaved()
    } catch (error) {
      toast.error(t("common.failedSave", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally { setSaving(false) }
  }

  return (
    <Dialog open={state.open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>{t(state.initial ? "vocabulary.editScheme" : "vocabulary.newScheme")}</DialogTitle></DialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-1.5"><Label>{t("vocabulary.schemeTitle")}</Label><Input value={title} onChange={(event) => setTitle(event.target.value)} autoFocus /></div>
          <div className="space-y-1.5"><Label>{t("common.description")}</Label><Textarea value={description} onChange={(event) => setDescription(event.target.value)} rows={3} /></div>
          <div className="space-y-1.5"><Label>{t("vocabulary.defaultLanguage")}</Label><Input value={language} onChange={(event) => setLanguage(event.target.value)} placeholder="zh-CN" /></div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
          <Button onClick={save} disabled={saving || !title.trim()}>{saving && <Loader2 className="h-4 w-4 animate-spin" />} {t("common.save")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function TermDialog({
  ksId, state, schemes, selectedSchemeIri, entityOptions, onOpenChange, onSaved,
}: {
  ksId: number
  state: { open: boolean; initial: VocabularyConcept | null }
  schemes: VocabularyScheme[]
  selectedSchemeIri: string
  entityOptions: ComboboxOption[]
  onOpenChange: (open: boolean) => void
  onSaved: () => void
}) {
  const { t } = useI18n()
  const [schemeIri, setSchemeIri] = useState("")
  const [preferred, setPreferred] = useState("")
  const [language, setLanguage] = useState("zh-CN")
  const [aliases, setAliases] = useState("")
  const [hidden, setHidden] = useState("")
  const [description, setDescription] = useState("")
  const [notation, setNotation] = useState("")
  const [broaderIri, setBroaderIri] = useState("")
  const [broaderQuery, setBroaderQuery] = useState("")
  const [broaderOptions, setBroaderOptions] = useState<ComboboxOption[]>([])
  const [broaderLoading, setBroaderLoading] = useState(false)
  const [mappedIri, setMappedIri] = useState<string | null>(null)
  const [status, setStatus] = useState<"active" | "deprecated">("active")
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!state.open) return
    const initial = state.initial
    const nextScheme = initial?.scheme_iri ?? selectedSchemeIri ?? schemes[0]?.iri ?? ""
    const defaultLanguage = schemes.find((scheme) => scheme.iri === nextScheme)?.default_language ?? "zh-CN"
    setSchemeIri(nextScheme)
    setPreferred(initial?.pref_labels[0]?.value ?? "")
    setLanguage(initial?.pref_labels[0]?.language || defaultLanguage)
    setAliases(initial?.alt_labels.map((label) => label.value).join(", ") ?? "")
    setHidden(initial?.hidden_labels.map((label) => label.value).join(", ") ?? "")
    setDescription(initial?.description ?? "")
    setNotation(initial?.notation ?? "")
    setBroaderIri(initial?.broader[0] ?? "")
    setBroaderQuery("")
    setMappedIri(initial?.mapped_entity_iri ?? null)
    setStatus(initial?.status ?? "active")
  }, [schemes, selectedSchemeIri, state])

  useEffect(() => {
    if (!state.open || !schemeIri) {
      setBroaderOptions([])
      setBroaderLoading(false)
      return
    }
    let cancelled = false
    const timer = setTimeout(async () => {
      setBroaderLoading(true)
      try {
        const result = await api.listVocabularyConcepts(ksId, {
          scheme_iri: schemeIri,
          q: broaderQuery.trim() || undefined,
          limit: 50,
        })
        if (cancelled) return
        const options = result.items
          .filter((concept) => concept.iri !== state.initial?.iri)
          .map((concept) => ({ value: concept.iri, label: concept.display_label }))
        const initialIndex = state.initial?.broader.indexOf(broaderIri) ?? -1
        const initialLabel = initialIndex >= 0
          ? state.initial?.broader_labels[initialIndex]
          : undefined
        setBroaderOptions((current) => {
          const selected = broaderIri
            ? current.find((option) => option.value === broaderIri) ?? {
              value: broaderIri,
              label: initialLabel ?? broaderIri.split(/[#/]/).pop() ?? broaderIri,
            }
            : null
          return [...new Map(
            [{ value: "", label: "—" }, ...(selected ? [selected] : []), ...options]
              .map((option) => [option.value, option]),
          ).values()]
        })
      } catch {
        if (!cancelled) {
          setBroaderOptions((current) => current.filter(
            (option) => option.value === "" || option.value === broaderIri,
          ))
        }
      } finally {
        if (!cancelled) setBroaderLoading(false)
      }
    }, 250)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [broaderIri, broaderQuery, ksId, schemeIri, state.initial, state.open])

  const save = async () => {
    if (!preferred.trim() || !schemeIri) return
    setSaving(true)
    const body: VocabularyConceptInput = {
      scheme_iri: schemeIri,
      pref_labels: [{ value: preferred.trim(), language: language.trim() || "zh-CN" }],
      alt_labels: splitLabels(aliases).map((value) => ({ value, language: language.trim() || "zh-CN" })),
      hidden_labels: splitLabels(hidden).map((value) => ({ value, language: language.trim() || "zh-CN" })),
      description: description.trim(), notation: notation.trim(),
      broader: broaderIri ? [broaderIri] : [], related: state.initial?.related ?? [],
      mapped_entity_iri: mappedIri || null, status,
    }
    try {
      if (state.initial) await api.updateVocabularyConcept(ksId, state.initial.iri, body)
      else await api.createVocabularyConcept(ksId, body)
      toast.success(t(state.initial ? "vocabulary.termUpdated" : "vocabulary.termCreated"))
      onSaved()
    } catch (error) {
      toast.error(t("common.failedSave", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally { setSaving(false) }
  }

  return (
    <Dialog open={state.open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader><DialogTitle>{t(state.initial ? "vocabulary.editTerm" : "vocabulary.newTerm")}</DialogTitle></DialogHeader>
        <div className="grid gap-4 py-2 sm:grid-cols-2">
          {schemes.length > 1 && <div className="space-y-1.5 sm:col-span-2">
            <Label>{t("vocabulary.scheme")}</Label>
            <Select value={schemeIri} onValueChange={(value) => {
              setSchemeIri(value)
              setBroaderIri("")
            }} disabled={!!state.initial}>
              <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>{schemes.map((scheme) => <SelectItem key={scheme.iri} value={scheme.iri}>{scheme.title}</SelectItem>)}</SelectContent>
            </Select>
          </div>}
          <div className="space-y-1.5"><Label>{t("vocabulary.preferredTerm")}</Label><Input value={preferred} onChange={(event) => setPreferred(event.target.value)} autoFocus /></div>
          <div className="space-y-1.5"><Label>{t("vocabulary.language")}</Label><Input value={language} onChange={(event) => setLanguage(event.target.value)} placeholder="zh-CN" /></div>
          <div className="space-y-1.5 sm:col-span-2"><Label>{t("vocabulary.alternativeLabels")}</Label><Input value={aliases} onChange={(event) => setAliases(event.target.value)} placeholder={t("vocabulary.aliasPlaceholder")} /></div>
          <div className="space-y-1.5 sm:col-span-2"><Label>{t("vocabulary.hiddenLabels")}</Label><Input value={hidden} onChange={(event) => setHidden(event.target.value)} placeholder={t("vocabulary.hiddenPlaceholder")} /></div>
          <div className="space-y-1.5 sm:col-span-2"><Label>{t("common.description")}</Label><Textarea value={description} onChange={(event) => setDescription(event.target.value)} rows={3} /></div>
          <div className="space-y-1.5"><Label>{t("vocabulary.notation")}</Label><Input value={notation} onChange={(event) => setNotation(event.target.value)} /></div>
          <div className="space-y-1.5">
            <Label>{t("common.status")}</Label>
            <Select value={status} onValueChange={(value) => setStatus(value as "active" | "deprecated")}>
              <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="active">{t("vocabulary.status.active")}</SelectItem>
                <SelectItem value="deprecated">{t("vocabulary.status.deprecated")}</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label>{t("vocabulary.broader")}</Label>
            <Combobox value={broaderIri} onChange={setBroaderIri} options={broaderOptions}
              placeholder="—" searchPlaceholder={t("vocabulary.searchTerms")}
              emptyText={t("common.noData")} loading={broaderLoading} loadingText={t("common.loading")}
              onSearchChange={setBroaderQuery} />
          </div>
          <div className="space-y-1.5">
            <Label>{t("vocabulary.mapping")}</Label>
            <Combobox value={mappedIri ?? ""} onChange={(value) => setMappedIri(value || null)} options={entityOptions}
              placeholder={t("vocabulary.noMapping")} searchPlaceholder={t("common.search")} />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
          <Button onClick={save} disabled={saving || !preferred.trim() || !schemeIri}>
            {saving && <Loader2 className="h-4 w-4 animate-spin" />} {t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
