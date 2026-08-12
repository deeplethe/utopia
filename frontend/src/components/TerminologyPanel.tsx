import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react"
import { Check, Loader2, X } from "lucide-react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import { useI18n, type MessageKey } from "@/lib/i18n"
import type { OntologyView, TermProposal, VocabularyLabel } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { Textarea } from "@/components/ui/textarea"
import {
  REVIEW_PAGE_SIZE, By, HoverText, ReviewActionButton, ReviewDetailSheet, ReviewPagination,
  ReviewProvenance, ReviewQueueHeader, ReviewStatusBadge, ReviewTableFrame,
  fmtWhen, matchesReviewFilters, type ReviewFilter,
} from "@/components/review-bits"

function splitLabels(value: string): string[] {
  return [...new Set(value.split(/[,，\n]+/).map((item) => item.trim()).filter(Boolean))]
}

function labelsFrom(payload: Record<string, unknown>, key: string): VocabularyLabel[] {
  const value = payload[key]
  if (!Array.isArray(value)) return []
  return value.flatMap((item) => {
    if (!item || typeof item !== "object") return []
    const label = item as Record<string, unknown>
    const text = typeof label.value === "string" ? label.value.trim() : ""
    if (!text) return []
    return [{ value: text, language: typeof label.language === "string" ? label.language : "" }]
  })
}

function irisFrom(payload: Record<string, unknown>, key: string): string[] {
  const value = payload[key]
  if (!Array.isArray(value)) return []
  return value.filter((item): item is string => typeof item === "string" && Boolean(item))
}

function stringFrom(payload: Record<string, unknown>, key: string): string {
  const value = payload[key]
  return typeof value === "string" ? value : ""
}

function actionKey(action: TermProposal["action"]): MessageKey {
  if (action === "create") return "review.terminology.action.create"
  if (action === "add_alias") return "review.terminology.action.addAlias"
  return "review.terminology.action.update"
}

function statusKey(status: TermProposal["status"]): MessageKey {
  if (status === "accepted") return "review.terminology.status.accepted"
  if (status === "rejected") return "review.terminology.status.rejected"
  return "review.terminology.status.pending"
}

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0 space-y-1">
      <p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{label}</p>
      <div className="min-w-0 break-words text-sm">{children || <span className="text-muted-foreground">—</span>}</div>
    </div>
  )
}

export default function TerminologyPanel({
  ksId, view, canWrite, onChanged,
}: {
  ksId: number
  view: OntologyView
  canWrite: boolean
  onChanged?: () => void
}) {
  const { t } = useI18n()
  const [items, setItems] = useState<TermProposal[]>([])
  const [conceptLabels, setConceptLabels] = useState<Record<string, string>>({})
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState("")
  const [filter, setFilter] = useState<ReviewFilter>("all")
  const [startDate, setStartDate] = useState("")
  const [endDate, setEndDate] = useState("")
  const [decisionMaker, setDecisionMaker] = useState<string | null>(null)
  const [page, setPage] = useState(0)
  const [selected, setSelected] = useState<TermProposal | null>(null)
  const [busy, setBusy] = useState(false)
  const [preferred, setPreferred] = useState("")
  const [aliases, setAliases] = useState("")
  const [language, setLanguage] = useState("zh-CN")
  const [note, setNote] = useState("")

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [proposals, vocabulary] = await Promise.all([
        api.listTermProposals(ksId, { status: "all", limit: 1000 }),
        api.getVocabulary(ksId),
      ])
      setItems(proposals.items)
      setConceptLabels(Object.fromEntries(vocabulary.concepts.map((concept) => [concept.iri, concept.display_label])))
    } catch (error) {
      toast.error(t("common.failedLoad", { error: (error as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [ksId, t])

  useEffect(() => { load() }, [load])
  useEffect(() => { setPage(0) }, [query, filter, startDate, endDate, decisionMaker])

  const iriLabels = useMemo(() => Object.fromEntries([
    ...Object.entries(conceptLabels),
    ...view.classes.map((entity) => [entity.iri, entity.label]),
    ...view.object_properties.map((entity) => [entity.iri, entity.label]),
    ...view.data_properties.map((entity) => [entity.iri, entity.label]),
  ]), [conceptLabels, view])

  const iriLabel = (iri: string) => iriLabels[iri] ?? iri
  const pendingCount = items.filter((item) => item.status === "pending").length
  const decisionMakers = useMemo(
    () => items.flatMap((item) => item.resolved_by ? [item.resolved_by] : []),
    [items],
  )
  const filtered = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase()
    return items.filter((item) => {
      if (filter === "pending" && item.status !== "pending") return false
      if (filter === "decided" && item.status === "pending") return false
      const when = item.status === "pending" ? item.created_at : item.resolved_at ?? item.created_at
      if (!matchesReviewFilters({ when, by: item.resolved_by, startDate, endDate, decisionMaker })) return false
      if (!normalized) return true
      return [
        item.term, item.target_label, item.target_iri, item.reason, item.resolution_note,
        JSON.stringify(item.payload), ...item.evidence.map((evidence) => `${evidence.document ?? ""} ${evidence.snippet}`),
      ].join(" ").toLocaleLowerCase().includes(normalized)
    })
  }, [decisionMaker, endDate, filter, items, query, startDate])
  const pageCount = Math.max(1, Math.ceil(filtered.length / REVIEW_PAGE_SIZE))
  const safePage = Math.min(page, pageCount - 1)
  const shown = filtered.slice(safePage * REVIEW_PAGE_SIZE, (safePage + 1) * REVIEW_PAGE_SIZE)

  const proposedSummary = (proposal: TermProposal) => {
    if (proposal.action === "add_alias") {
      const target = proposal.target_label ?? (proposal.target_iri ? iriLabels[proposal.target_iri] : "")
      return target
        ? t("review.terminology.aliasFor", { name: target })
        : t(actionKey(proposal.action))
    }
    const parts: string[] = []
    const alternate = labelsFrom(proposal.payload, "alt_labels").map((label) => label.value)
    if (alternate.length) parts.push(t("review.terminology.proposedAliases", { aliases: alternate.join(", ") }))
    const broader = stringFrom(proposal.payload, "broader_iri") || irisFrom(proposal.payload, "broader")[0]
    if (broader) parts.push(t("review.terminology.proposedBroader", { name: iriLabel(broader) }))
    const mapping = stringFrom(proposal.payload, "mapped_entity_iri")
    if (mapping) parts.push(t("review.terminology.proposedMapping", { name: iriLabel(mapping) }))
    return parts.join(" · ") || t(actionKey(proposal.action))
  }

  const openProposal = (proposal: TermProposal) => {
    const sourceLabels = proposal.action === "add_alias"
      ? labelsFrom(proposal.payload, "add_alt_labels")
      : labelsFrom(proposal.payload, "alt_labels")
    const preferredLabels = labelsFrom(proposal.payload, "pref_labels")
    setPreferred(preferredLabels[0]?.value ?? proposal.term)
    setAliases(sourceLabels.map((label) => label.value).join(", "))
    setLanguage(preferredLabels[0]?.language || sourceLabels[0]?.language || "zh-CN")
    setNote(proposal.resolution_note ?? "")
    setSelected(proposal)
  }

  const decide = async (decision: "accept" | "reject") => {
    if (!selected) return
    setBusy(true)
    try {
      if (decision === "accept") {
        let payload = selected.payload
        if (selected.action === "create") {
          payload = {
            ...selected.payload,
            pref_labels: [{ value: preferred.trim(), language: language.trim() || "zh-CN" }],
            alt_labels: splitLabels(aliases).map((value) => ({ value, language: language.trim() || "zh-CN" })),
          }
        } else if (selected.action === "add_alias") {
          payload = {
            ...selected.payload,
            add_alt_labels: splitLabels(aliases).map((value) => ({ value, language: language.trim() || "zh-CN" })),
          }
        }
        await api.acceptTermProposal(ksId, selected.id, payload, note.trim())
        toast.success(t("review.terminology.accepted"))
      } else {
        await api.rejectTermProposal(ksId, selected.id, note.trim())
        toast.success(t("review.terminology.rejected"))
      }
      setSelected(null)
      await load()
      onChanged?.()
    } catch (error) {
      toast.error(t(decision === "accept" ? "review.terminology.acceptFailed" : "review.terminology.rejectFailed", {
        error: (error as Error).message.replace(/^\d+:\s*/, ""),
      }))
    } finally {
      setBusy(false)
    }
  }

  const selectedBroader = selected
    ? stringFrom(selected.payload, "broader_iri") || irisFrom(selected.payload, "broader")[0] || ""
    : ""
  const selectedRelated = selected ? irisFrom(selected.payload, "related") : []
  const selectedMapping = selected ? stringFrom(selected.payload, "mapped_entity_iri") : ""
  const selectedHidden = selected ? labelsFrom(selected.payload, "hidden_labels") : []
  const selectedScheme = selected ? stringFrom(selected.payload, "scheme_iri") : ""
  const canAccept = Boolean(selected) && (
    selected?.action === "update"
    || (selected?.action === "create" && Boolean(preferred.trim()))
    || (selected?.action === "add_alias" && splitLabels(aliases).length > 0)
  )

  return (
    <div className="space-y-4">
      <ReviewQueueHeader
        title={t("review.terminology.title")}
        query={query}
        onQueryChange={setQuery}
        filter={filter}
        onFilterChange={setFilter}
        pendingCount={pendingCount}
        startDate={startDate}
        onStartDateChange={setStartDate}
        endDate={endDate}
        onEndDateChange={setEndDate}
        decisionMaker={decisionMaker}
        onDecisionMakerChange={setDecisionMaker}
        decisionMakers={decisionMakers}
        onReset={() => {
          setQuery("")
          setFilter("all")
          setStartDate("")
          setEndDate("")
          setDecisionMaker(null)
        }}
        onRefresh={load}
        refreshing={loading}
      />

      <ReviewTableFrame>
        <Table className="table-fixed">
          <TableHeader>
            <TableRow>
              <TableHead className="w-[28%]">{t("review.subject")}</TableHead>
              <TableHead className="w-36">{t("common.status")}</TableHead>
              <TableHead>{t("review.issueRationale")}</TableHead>
              <TableHead className="w-40">{t("review.provenance")}</TableHead>
              <TableHead className="w-24" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={5} className="h-20 text-center text-muted-foreground">{t("common.loading")}</TableCell></TableRow>
            ) : shown.length === 0 ? (
              <TableRow><TableCell colSpan={5} className="h-20 text-center text-muted-foreground">{t("common.noData")}</TableCell></TableRow>
            ) : shown.map((proposal) => {
              const decided = proposal.status !== "pending"
              const targetName = proposal.target_label ?? (proposal.target_iri ? iriLabels[proposal.target_iri] : "")
              return (
                <TableRow key={proposal.id} className={proposal.status === "pending" ? "bg-amber-500/5" : undefined}>
                  <TableCell className="max-w-[20rem]">
                    <div className="whitespace-normal break-words font-medium">{proposal.term}</div>
                    <Badge variant="outline" className="mt-1 text-[10px]">{t(actionKey(proposal.action))}</Badge>
                    <HoverText text={proposedSummary(proposal)} className="mt-1 text-xs text-muted-foreground" />
                  </TableCell>
                  <TableCell>
                    <ReviewStatusBadge tone={proposal.status === "pending" ? "pending" : proposal.status === "accepted" ? "success" : "neutral"}>
                      {t(statusKey(proposal.status))}
                    </ReviewStatusBadge>
                  </TableCell>
                  <TableCell className="max-w-[24rem] text-xs">
                    {targetName ? (
                      <div className="min-w-0 space-y-1">
                        <p className="break-words font-medium [overflow-wrap:anywhere]">{targetName}</p>
                        <p className="whitespace-pre-wrap break-words leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">
                          {proposal.reason || "—"}
                        </p>
                      </div>
                    ) : (
                      <p className="whitespace-pre-wrap break-words leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">
                        {proposal.reason || "—"}
                      </p>
                    )}
                  </TableCell>
                  <TableCell>
                    <ReviewProvenance
                      by={decided ? proposal.resolved_by : proposal.proposed_by}
                      when={decided ? proposal.resolved_at : proposal.created_at}
                      meta={proposal.extraction_job_id ? (
                        <p className="text-[11px] text-muted-foreground">{t("review.terminology.extractionJob")} #{proposal.extraction_job_id}</p>
                      ) : undefined}
                    />
                  </TableCell>
                  <TableCell className="text-right">
                    <ReviewActionButton onClick={() => openProposal(proposal)} />
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      </ReviewTableFrame>

      <ReviewPagination page={safePage} total={filtered.length} onPageChange={setPage} />

      {selected && (
        <ReviewDetailSheet
          open
          onOpenChange={(open) => { if (!open && !busy) setSelected(null) }}
          badges={(
            <>
              <ReviewStatusBadge tone={selected.status === "pending" ? "pending" : selected.status === "accepted" ? "success" : "neutral"}>
                {t(statusKey(selected.status))}
              </ReviewStatusBadge>
              <Badge variant="secondary">{t(actionKey(selected.action))}</Badge>
            </>
          )}
          title={selected.term}
          footer={selected.status === "pending" && canWrite ? (
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={() => decide("reject")} disabled={busy}>
                {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <X className="h-4 w-4" />} {t("review.terminology.reject")}
              </Button>
              <Button onClick={() => decide("accept")} disabled={busy || !canAccept}>
                {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Check className="h-4 w-4" />} {t("review.terminology.accept")}
              </Button>
            </div>
          ) : <Button variant="outline" onClick={() => setSelected(null)}>{t("common.close")}</Button>}
        >
                <div className="grid gap-4 rounded-lg border bg-muted/20 p-4 sm:grid-cols-2">
                  <Detail label={t("review.terminology.proposedBy")}><By by={selected.proposed_by} /></Detail>
                  <Detail label={t("review.terminology.submittedAt")}>{fmtWhen(selected.created_at)}</Detail>
                  {selected.extraction_job_id && <Detail label={t("review.terminology.extractionJob")}>#{selected.extraction_job_id}</Detail>}
                  {selected.resolved_by && <Detail label={t("review.by")}><By by={selected.resolved_by} /></Detail>}
                  {selected.resolved_at && <Detail label={t("review.terminology.resolvedAt")}>{fmtWhen(selected.resolved_at)}</Detail>}
                </div>

                <section className="space-y-2">
                  <h3 className="text-sm font-semibold">{t("common.reason")}</h3>
                  <p className="whitespace-pre-wrap rounded-lg border p-3 text-sm leading-relaxed text-muted-foreground">{selected.reason || "—"}</p>
                </section>

                <section className="space-y-3">
                  <h3 className="text-sm font-semibold">{t("review.terminology.suggestion")}</h3>
                  <div className="grid gap-4 rounded-lg border p-4 sm:grid-cols-2">
                    {selected.action === "create" && selected.status === "pending" && canWrite ? (
                      <>
                        <div className="space-y-2"><Label>{t("vocabulary.preferredTerm")}</Label><Input value={preferred} onChange={(event) => setPreferred(event.target.value)} /></div>
                        <div className="space-y-2"><Label>{t("vocabulary.language")}</Label><Input value={language} onChange={(event) => setLanguage(event.target.value)} /></div>
                        <div className="space-y-2 sm:col-span-2"><Label>{t("vocabulary.alternativeLabels")}</Label><Input value={aliases} onChange={(event) => setAliases(event.target.value)} placeholder={t("vocabulary.aliasPlaceholder")} /></div>
                      </>
                    ) : (
                      <Detail label={t("vocabulary.preferredTerm")}>{preferred}</Detail>
                    )}
                    {selected.action === "add_alias" && selected.status === "pending" && canWrite ? (
                      <>
                        <div className="space-y-2 sm:col-span-2"><Label>{t("vocabulary.alternativeLabels")}</Label><Input value={aliases} onChange={(event) => setAliases(event.target.value)} placeholder={t("vocabulary.aliasPlaceholder")} /></div>
                        <div className="space-y-2"><Label>{t("vocabulary.language")}</Label><Input value={language} onChange={(event) => setLanguage(event.target.value)} /></div>
                      </>
                    ) : selected.action === "add_alias" ? <Detail label={t("vocabulary.alternativeLabels")}>{aliases || "—"}</Detail> : null}
                    {selected.target_iri && <Detail label={t("review.terminology.target")}><span>{selected.target_label ?? iriLabel(selected.target_iri)}</span><code className="mt-1 block break-all text-[11px] text-muted-foreground">{selected.target_iri}</code></Detail>}
                    {selectedScheme && <Detail label={t("vocabulary.scheme")}><code className="break-all text-xs">{selectedScheme}</code></Detail>}
                    {selectedHidden.length > 0 && <Detail label={t("vocabulary.hiddenLabels")}>{selectedHidden.map((label) => label.value).join(", ")}</Detail>}
                    {stringFrom(selected.payload, "description") && <Detail label={t("common.description")}>{stringFrom(selected.payload, "description")}</Detail>}
                    {stringFrom(selected.payload, "notation") && <Detail label={t("vocabulary.notation")}>{stringFrom(selected.payload, "notation")}</Detail>}
                    {selectedBroader && <Detail label={t("vocabulary.broader")}><span>{iriLabel(selectedBroader)}</span><code className="mt-1 block break-all text-[11px] text-muted-foreground">{selectedBroader}</code></Detail>}
                    {selectedRelated.length > 0 && <Detail label={t("vocabulary.related")}>{selectedRelated.map(iriLabel).join(", ")}</Detail>}
                    {selectedMapping && <Detail label={t("vocabulary.mapping")}><span>{iriLabel(selectedMapping)}</span><code className="mt-1 block break-all text-[11px] text-muted-foreground">{selectedMapping}</code></Detail>}
                  </div>
                  <details className="rounded-lg border px-3 py-2">
                    <summary className="cursor-pointer text-xs font-medium text-muted-foreground">{t("review.terminology.technicalPayload")}</summary>
                    <pre className="mt-3 max-h-56 overflow-auto whitespace-pre-wrap break-all rounded bg-muted p-3 text-[11px]">{JSON.stringify(selected.payload, null, 2)}</pre>
                  </details>
                </section>

                <section className="space-y-3">
                  <h3 className="text-sm font-semibold">{t("review.terminology.evidence")}</h3>
                  {selected.evidence.length === 0 ? (
                    <p className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">{t("review.terminology.noEvidence")}</p>
                  ) : selected.evidence.map((evidence) => (
                    <article key={`${evidence.chunk_id}-${evidence.document_id ?? "deleted"}`} className="space-y-2 rounded-lg border p-3">
                      <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
                        <span className="font-medium">{evidence.document ?? t("review.deletedSource")}</span>
                        <Badge variant="secondary">{t("review.chunk", { index: evidence.chunk_id })}</Badge>
                      </div>
                      <p className="whitespace-pre-wrap break-words text-xs leading-relaxed text-muted-foreground">{evidence.snippet}</p>
                    </article>
                  ))}
                </section>

                {(selected.status === "pending" && canWrite) ? (
                  <div className="space-y-2">
                    <Label>{t("review.terminology.note")}</Label>
                    <Textarea value={note} onChange={(event) => setNote(event.target.value)} placeholder={t("review.terminology.notePlaceholder")} maxLength={1000} />
                  </div>
                ) : selected.resolution_note ? (
                  <Detail label={t("review.terminology.note")}>{selected.resolution_note}</Detail>
                ) : null}
        </ReviewDetailSheet>
      )}
    </div>
  )
}
