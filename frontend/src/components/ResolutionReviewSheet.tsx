import { useEffect, useState } from "react"
import { Ban, Check, Clock, Hash, Merge, Search, Sparkles } from "lucide-react"
import { api } from "@/lib/api"
import type { IndividualSummary, ResolutionQueueItem } from "@/lib/types"
import { useI18n } from "@/lib/i18n"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
import { fmtWhen, ReviewDetailSheet, ReviewStatusBadge } from "@/components/review-bits"

type Decision = "match" | "new" | "reject" | "defer"
type ResolveOptions = { iri?: string; reason?: string; reviewAfter?: string }

export default function ResolutionReviewSheet({
  ksId, item, canWrite, busy, onClose, onResolve, onMerge,
}: {
  ksId: number
  item: ResolutionQueueItem | null
  canWrite: boolean
  busy: boolean
  onClose: () => void
  onResolve: (action: Decision, options?: ResolveOptions) => void
  onMerge: (sourceIri: string, canonicalIri: string, reason: string) => void
}) {
  if (!item) return null
  return <ResolutionReviewContent {...{ ksId, item, canWrite, busy, onClose, onResolve, onMerge }} />
}

function ResolutionReviewContent({
  ksId, item, canWrite, busy, onClose, onResolve, onMerge,
}: {
  ksId: number
  item: ResolutionQueueItem
  canWrite: boolean
  busy: boolean
  onClose: () => void
  onResolve: (action: Decision, options?: ResolveOptions) => void
  onMerge: (sourceIri: string, canonicalIri: string, reason: string) => void
}) {
  const { t } = useI18n()
  const candidates = [...item.candidates].sort((left, right) => right.score - left.score)
  const [query, setQuery] = useState("")
  const [results, setResults] = useState<IndividualSummary[]>([])
  const [searching, setSearching] = useState(false)
  const [reason, setReason] = useState("")
  const [reviewAfter, setReviewAfter] = useState("")
  const [mergeSource, setMergeSource] = useState<IndividualSummary | null>(null)

  useEffect(() => {
    const term = query.trim()
    if (!term) { setResults([]); return }
    let cancelled = false
    const timer = window.setTimeout(async () => {
      setSearching(true)
      try {
        const response = await api.aboxIndividuals(ksId, { q: term, limit: 20 })
        if (!cancelled) setResults(response.items)
      } catch {
        if (!cancelled) setResults([])
      } finally {
        if (!cancelled) setSearching(false)
      }
    }, 250)
    return () => { cancelled = true; window.clearTimeout(timer) }
  }, [ksId, query])

  const reasonRequired = reason.trim().length === 0

  return (
    <ReviewDetailSheet
      open
      onOpenChange={(open) => { if (!open && !busy) onClose() }}
      badges={<><ReviewStatusBadge tone="pending">{t("common.pending")}</ReviewStatusBadge>{item.class_label && <Badge variant="secondary">{item.class_label}</Badge>}<span className="text-xs text-muted-foreground">#{item.id}</span></>}
      title={item.surface_form}
      description={t("review.resolution.ambiguity", { count: candidates.length })}
    >
      <section className="grid gap-3 rounded-lg border bg-muted/20 p-4 sm:grid-cols-2">
        <div><p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{t("common.classes")}</p><p className="mt-1 text-sm">{item.class_label ?? "—"}</p></div>
        <div><p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{t("review.when")}</p><p className="mt-1 text-sm">{fmtWhen(item.created_at)}</p></div>
        {item.source_chunk_id != null && <div className="sm:col-span-2"><p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{t("review.resolution.sourceChunk")}</p><p className="mt-1 flex items-center gap-1.5 text-sm"><Hash className="h-3.5 w-3.5" /> {item.source_chunk_id}</p></div>}
        {(item.reason || item.evidence) && <p className="sm:col-span-2 text-xs leading-relaxed text-muted-foreground">{item.reason || item.evidence}</p>}
      </section>

      <section className="space-y-3">
        <div><h3 className="text-sm font-semibold">{t("review.resolution.candidateOptions")}</h3><p className="text-xs text-muted-foreground">{t("review.resolution.candidateOptionsDescription")}</p></div>
        {candidates.length === 0 ? <div className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">{t("review.resolution.noCandidates")}</div> : <div className="space-y-2.5">{candidates.map((candidate) => (
          <div key={candidate.iri} className="flex items-start justify-between gap-3 rounded-lg border p-3.5">
            <div className="min-w-0"><span className="font-medium">{candidate.label}</span><code className="mt-2 block break-all text-[11px] leading-relaxed text-muted-foreground">{candidate.iri}</code></div>
            {canWrite && <div className="flex shrink-0 gap-1"><Button size="sm" disabled={busy} onClick={() => onResolve("match", { iri: candidate.iri })}><Check className="h-3.5 w-3.5" /> {t("review.resolution.link")}</Button><Button size="sm" variant="outline" disabled={busy} onClick={() => setMergeSource({ iri: candidate.iri, label: candidate.label, types: [] })}><Merge className="h-3.5 w-3.5" /> {t("review.resolution.merge")}</Button></div>}
          </div>
        ))}</div>}
      </section>

      <section className="space-y-3 rounded-lg border p-4">
        <div><h3 className="flex items-center gap-2 text-sm font-semibold"><Search className="h-4 w-4" /> {t("review.resolution.searchAll")}</h3><p className="text-xs text-muted-foreground">{mergeSource ? t("review.resolution.mergeSource", { name: mergeSource.label }) : t("review.resolution.searchAllDescription")}</p></div>
        <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("review.resolution.searchPlaceholder")} />
        {searching && <p className="text-xs text-muted-foreground">{t("common.loading")}</p>}
        {results.map((individual) => <div key={individual.iri} className="flex items-center justify-between gap-3 rounded-md border p-2.5"><div className="min-w-0"><p className="text-sm font-medium">{individual.label}</p><code className="block truncate text-[10px] text-muted-foreground">{individual.iri}</code></div>{canWrite && (!mergeSource ? <Button size="sm" disabled={busy} onClick={() => onResolve("match", { iri: individual.iri })}>{t("review.resolution.link")}</Button> : individual.iri !== mergeSource.iri ? <Button size="sm" variant="destructive" disabled={busy || reasonRequired} onClick={() => onMerge(mergeSource.iri, individual.iri, reason.trim())}>{t("review.resolution.mergeInto")}</Button> : null)}</div>)}
        {mergeSource && <Button size="sm" variant="ghost" onClick={() => setMergeSource(null)}>{t("common.cancel")}</Button>}
      </section>

      {canWrite && <section className="space-y-3 rounded-lg border border-dashed p-4">
        <Textarea value={reason} onChange={(event) => setReason(event.target.value)} placeholder={t("review.resolution.reasonPlaceholder")} />
        <div className="flex flex-wrap items-center gap-2">
          <Button variant="outline" disabled={busy} onClick={() => onResolve("new", { reason: reason.trim() })}><Sparkles className="h-4 w-4" /> {t("review.newIndividual")}</Button>
          <Button variant="outline" disabled={busy || reasonRequired} onClick={() => onResolve("reject", { reason: reason.trim() })}><Ban className="h-4 w-4" /> {t("review.resolution.reject")}</Button>
          <Input className="w-auto" type="datetime-local" value={reviewAfter} onChange={(event) => setReviewAfter(event.target.value)} />
          <Button variant="outline" disabled={busy || reasonRequired} onClick={() => onResolve("defer", { reason: reason.trim(), reviewAfter: reviewAfter ? new Date(reviewAfter).toISOString() : undefined })}><Clock className="h-4 w-4" /> {t("review.resolution.defer")}</Button>
        </div>
      </section>}
      {!canWrite && <p className="rounded-lg border p-3 text-xs text-muted-foreground">{t("review.readOnly")}</p>}
    </ReviewDetailSheet>
  )
}
