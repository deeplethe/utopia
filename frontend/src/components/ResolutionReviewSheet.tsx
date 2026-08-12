import { Check, Hash, Sparkles } from "lucide-react"
import type { ResolutionQueueItem } from "@/lib/types"
import { useI18n } from "@/lib/i18n"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { fmtWhen, ReviewDetailSheet, ReviewStatusBadge } from "@/components/review-bits"

export default function ResolutionReviewSheet({
  item, canWrite, busy, onClose, onResolve,
}: {
  item: ResolutionQueueItem | null
  canWrite: boolean
  busy: boolean
  onClose: () => void
  onResolve: (action: "match" | "new", iri?: string) => void
}) {
  const { t } = useI18n()
  if (!item) return null
  const candidates = [...item.candidates].sort((left, right) => right.score - left.score)

  return (
    <ReviewDetailSheet
      open
      onOpenChange={(open) => { if (!open && !busy) onClose() }}
      badges={(
        <>
          <ReviewStatusBadge tone="pending">{t("common.pending")}</ReviewStatusBadge>
          {item.class_label && <Badge variant="secondary">{item.class_label}</Badge>}
          <span className="text-xs text-muted-foreground">#{item.id}</span>
        </>
      )}
      title={item.surface_form}
      description={t("review.resolution.ambiguity", { count: candidates.length })}
    >
      <section className="grid gap-3 rounded-lg border bg-muted/20 p-4 sm:grid-cols-2">
        <div>
          <p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{t("common.classes")}</p>
          <p className="mt-1 text-sm">{item.class_label ?? "—"}</p>
        </div>
        <div>
          <p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{t("review.when")}</p>
          <p className="mt-1 text-sm">{fmtWhen(item.created_at)}</p>
        </div>
        {item.source_chunk_id != null && (
          <div className="sm:col-span-3">
            <p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{t("review.resolution.sourceChunk")}</p>
            <p className="mt-1 flex items-center gap-1.5 text-sm"><Hash className="h-3.5 w-3.5" /> {item.source_chunk_id}</p>
          </div>
        )}
      </section>

      <section className="space-y-3">
        <div>
          <h3 className="text-sm font-semibold">{t("review.resolution.candidateOptions")}</h3>
          <p className="text-xs text-muted-foreground">{t("review.resolution.candidateOptionsDescription")}</p>
        </div>
        {candidates.length === 0 ? (
          <div className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">{t("review.resolution.noCandidates")}</div>
        ) : (
          <div className="space-y-2.5">
            {candidates.map((candidate) => (
              <div key={candidate.iri} className="flex items-start justify-between gap-3 rounded-lg border p-3.5">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">{candidate.label}</span>
                  </div>
                  <code className="mt-2 block break-all text-[11px] leading-relaxed text-muted-foreground">{candidate.iri}</code>
                </div>
                {canWrite && (
                  <Button size="sm" className="shrink-0" disabled={busy} onClick={() => onResolve("match", candidate.iri)}>
                    <Check className="h-3.5 w-3.5" /> {t("common.apply")}
                  </Button>
                )}
              </div>
            ))}
          </div>
        )}
      </section>

      <section className="flex items-start justify-between gap-3 rounded-lg border border-dashed p-4">
        <div>
          <h3 className="flex items-center gap-2 text-sm font-semibold"><Sparkles className="h-4 w-4" /> {t("review.newIndividual")}</h3>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{t("review.resolution.createDescription")}</p>
        </div>
        {canWrite && (
          <Button variant="outline" className="shrink-0" disabled={busy} onClick={() => onResolve("new")}>
            {t("review.newIndividual")}
          </Button>
        )}
      </section>

      {!canWrite && <p className="rounded-lg border p-3 text-xs text-muted-foreground">{t("review.readOnly")}</p>}
    </ReviewDetailSheet>
  )
}
