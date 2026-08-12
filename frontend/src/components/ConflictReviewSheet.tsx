import { useEffect, useMemo, useState } from "react"
import {
  Check,
  Copy,
  FileText,
  Loader2,
  Sparkles,
  X,
} from "lucide-react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import { useI18n, type Translate } from "@/lib/i18n"
import { conflictSubject, conflictTypeLabel } from "@/lib/conflicts"
import type { Conflict, ConflictContext, ConflictEvidenceSource, ConflictResolution, EditOp } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ReviewDetailSheet, ReviewStatusBadge } from "@/components/review-bits"

function entityRole(conflict: Conflict, index: number, t: Translate) {
  if (conflict.ctype === "domain_multi") return index === 0 ? t("review.role.property") : t("review.role.currentDomain")
  if (conflict.ctype === "range_multi") return index === 0 ? t("review.role.property") : t("review.role.currentRange")
  if (conflict.ctype === "duplicate") return t("review.role.candidate", { name: String.fromCharCode(65 + index) })
  if (conflict.ctype === "cycle") return t("review.role.cycleMember")
  if (conflict.ctype === "predicate_specialization") return t("review.role.relation")
  return t("review.role.affectedEntity")
}

function localName(iri: string) {
  return iri.split(/[/#]/).at(-1) || iri
}

function operationSummary(operation: EditOp, labels: Map<string, string>, t: Translate) {
  const label = (value: unknown) => {
    if (typeof value !== "string") return String(value ?? "")
    return labels.get(value) ?? localName(value)
  }
  switch (operation.op) {
    case "update_property": {
      const slot = "domain" in operation ? "domain" : "range"
      const slotLabel = t(slot === "domain" ? "review.slot.domain" : "review.slot.range")
      return t("review.operation.setProperty", { property: label(operation.iri), slot: slotLabel, value: label(operation[slot]) })
    }
    case "set_property_union": {
      const members = Array.isArray(operation.members) ? operation.members.map(label).join(" ∪ ") : ""
      const slot = operation.slot === "domain" ? t("review.slot.domain") : t("review.slot.range")
      return t("review.operation.setUnion", { slot, members })
    }
    case "delete_axiom": {
      if (operation.type === "subclass") return t("review.operation.deleteSubclass", { left: label(operation.sub), right: label(operation.super) })
      if (operation.type === "disjoint") return t("review.operation.deleteDisjoint", { left: label(operation.a), right: label(operation.b) })
      if (operation.type === "equivalent") return t("review.operation.deleteEquivalent", { left: label(operation.a), right: label(operation.b) })
      return t("review.operation.deleteAxiom")
    }
    case "merge_classes":
      return t("review.operation.mergeClasses", { source: label(operation.source), target: label(operation.target) })
    case "merge_properties": {
      const sources = Array.isArray(operation.sources) ? operation.sources.map(label).join(", ") : t("review.operation.sourceRelations")
      return t("review.operation.mergeProperties", { sources, target: label(operation.target ?? operation.target_label) })
    }
    case "subordinate_properties": {
      const sources = Array.isArray(operation.sources) ? operation.sources.map(label).join(", ") : t("review.operation.sourceRelations")
      return t("review.operation.subordinateProperties", { sources, target: label(operation.target_label) })
    }
    default:
      return t("review.operation.applyEdit", { operation: operation.op })
  }
}

async function copyIri(iri: string, t: Translate) {
  try {
    await navigator.clipboard.writeText(iri)
    toast.success(t("review.iriCopied"))
  } catch {
    toast.error(t("review.copyIriFailed"))
  }
}

function ResolutionOption({
  resolution,
  recommended,
  recommendationReason,
  labels,
  canWrite,
  busy,
  onResolve,
}: {
  resolution: ConflictResolution
  recommended: boolean
  recommendationReason: string | null
  labels: Map<string, string>
  canWrite: boolean
  busy: boolean
  onResolve: () => void
}) {
  const { t } = useI18n()
  const suggestsDeletion = resolution.op.op === "delete_axiom"
  return (
    <div className={`rounded-lg border p-3.5 ${recommended ? "border-primary/40 bg-primary/5" : "bg-card"}`}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 space-y-1.5">
          <div className="flex flex-wrap items-center gap-2">
            <span className={`font-medium ${suggestsDeletion ? "text-muted-foreground line-through decoration-1" : ""}`}>
              {resolution.label}
            </span>
            {recommended && (
              <Badge variant="outline" className="gap-1 border-primary/30 text-primary">
                <Sparkles className="h-3 w-3" /> {t("review.agentRecommended")}
              </Badge>
            )}
          </div>
          <p className="text-sm leading-relaxed text-muted-foreground">
            {operationSummary(resolution.op, labels, t)}
          </p>
          {recommended && recommendationReason && (
            <p className="text-xs leading-relaxed text-primary/90">{recommendationReason}</p>
          )}
        </div>
        {canWrite && (
          <Button size="sm" className="shrink-0" disabled={busy} onClick={onResolve}>
            {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Check className="h-3.5 w-3.5" />}
            {t("common.apply")}
          </Button>
        )}
      </div>
      <details className="mt-2 text-xs text-muted-foreground">
        <summary className="cursor-pointer select-none hover:text-foreground">{t("review.technicalOperation")}</summary>
        <pre className="mt-2 whitespace-pre-wrap break-all rounded-md bg-muted p-2 font-mono text-[11px] leading-relaxed">
          {JSON.stringify(resolution.op, null, 2)}
        </pre>
      </details>
    </div>
  )
}

export default function ConflictReviewSheet({
  ksId,
  conflict,
  canWrite,
  busy,
  onClose,
  onResolve,
  onDismiss,
}: {
  ksId: number
  conflict: Conflict | null
  canWrite: boolean
  busy: boolean
  onClose: () => void
  onResolve: (resolutionId: string) => void
  onDismiss: () => void
}) {
  const { t } = useI18n()
  const [context, setContext] = useState<ConflictContext | null>(null)
  const [contextError, setContextError] = useState<string | null>(null)

  useEffect(() => {
    if (!conflict) {
      setContext(null)
      setContextError(null)
      return
    }
    let cancelled = false
    setContext(null)
    setContextError(null)
    api.getConflictContext(ksId, conflict.id)
      .then((result) => { if (!cancelled) setContext(result) })
      .catch((error: Error) => { if (!cancelled) setContextError(error.message.replace(/^\d+:\s*/, "")) })
    return () => { cancelled = true }
  }, [ksId, conflict])

  const labels = useMemo(
    () => new Map(conflict?.payload.entities.map((entity) => [entity.iri, entity.label]) ?? []),
    [conflict],
  )
  const evidenceSources = useMemo(() => {
    const byChunk = new Map<number, { source: ConflictEvidenceSource; axioms: string[] }>()
    for (const axiom of context?.evidence ?? []) {
      for (const source of axiom.sources) {
        const current = byChunk.get(source.chunk_id)
        if (current) {
          if (!current.axioms.includes(axiom.description)) current.axioms.push(axiom.description)
        } else {
          byChunk.set(source.chunk_id, { source, axioms: [axiom.description] })
        }
      }
    }
    return [...byChunk.values()].sort((left, right) =>
      (left.source.document ?? "").localeCompare(right.source.document ?? "")
      || left.source.chunk_index - right.source.chunk_index)
  }, [context])

  if (!conflict) return null
  const recommendation = conflict.payload.recommendation
  const orderedResolutions = [...conflict.payload.resolutions].sort(
    (left, right) => Number(right.id === recommendation?.resolution_id) - Number(left.id === recommendation?.resolution_id),
  )

  return (
    <ReviewDetailSheet
      open
      onOpenChange={(open) => { if (!open && !busy) onClose() }}
      badges={(
        <>
          <ReviewStatusBadge
            tone={conflict.severity === "error" ? "error" : "warning"}
            title={conflict.severity === "error" ? t("common.error") : t("common.warning")}
          >
            {t("common.pending")}
          </ReviewStatusBadge>
          <Badge variant="secondary">{conflictTypeLabel(conflict.ctype, t)}</Badge>
          <span className="text-xs text-muted-foreground">{t("review.conflictNumber", { id: conflict.id })}</span>
        </>
      )}
      title={conflictSubject(conflict)}
      description={conflict.detail}
    >
          <section className="space-y-2.5">
            <div className="flex items-center justify-between">
              <h3 className="text-sm font-semibold">{t("review.affectedOntologyEntities")}</h3>
              <span className="text-xs text-muted-foreground">{t("review.entities", { count: conflict.payload.entities.length })}</span>
            </div>
            <div className="divide-y rounded-lg border">
              {conflict.payload.entities.map((entity, index) => (
                <div key={entity.iri} className="space-y-1.5 p-3">
                  <div className="flex items-center justify-between gap-3">
                    <div className="min-w-0">
                      <div className="font-medium">{entity.label}</div>
                      <div className="text-xs text-muted-foreground">{entityRole(conflict, index, t)}</div>
                    </div>
                    <Button size="icon" variant="ghost" className="h-7 w-7 shrink-0" title={t("review.copyIri")} onClick={() => copyIri(entity.iri, t)}>
                      <Copy className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                  <code className="block whitespace-normal break-all rounded bg-muted px-2 py-1.5 text-[11px] leading-relaxed text-muted-foreground">
                    {entity.iri}
                  </code>
                </div>
              ))}
            </div>
          </section>

          <section className="space-y-2.5">
            <div>
              <h3 className="text-sm font-semibold">{t("review.resolutionOptions")}</h3>
              <p className="text-xs text-muted-foreground">{t("review.resolutionDescription")}</p>
            </div>
            <div className="space-y-2.5">
              {orderedResolutions.map((resolution) => (
                <ResolutionOption
                  key={resolution.id}
                  resolution={resolution}
                  recommended={resolution.id === recommendation?.resolution_id}
                  recommendationReason={resolution.id === recommendation?.resolution_id ? recommendation?.reason ?? null : null}
                  labels={labels}
                  canWrite={canWrite}
                  busy={busy}
                  onResolve={() => onResolve(resolution.id)}
                />
              ))}
            </div>
          </section>

          <section className="space-y-2.5">
            <div>
              <h3 className="flex items-center gap-2 text-sm font-semibold">
                <FileText className="h-4 w-4" /> {t("review.sourceEvidence")}
              </h3>
              <p className="text-xs text-muted-foreground">{t("review.sourceEvidenceDescription")}</p>
            </div>
            {!context && !contextError ? (
              <div className="flex items-center gap-2 rounded-lg border p-4 text-sm text-muted-foreground">
                <Loader2 className="h-4 w-4 animate-spin" /> {t("review.loadingProvenance")}
              </div>
            ) : contextError ? (
              <div className="rounded-lg border border-destructive/20 bg-destructive/5 p-4 text-sm text-destructive">{contextError}</div>
            ) : context?.evidence.length ? (
              <div className="space-y-3">
                <div className="rounded-lg border">
                  <div className="border-b px-3.5 py-2.5 text-xs font-medium text-muted-foreground">{t("review.conflictingAxioms")}</div>
                  <div className="divide-y">
                    {context.evidence.map((axiom) => (
                      <div key={axiom.axiom_key} className="px-3.5 py-3">
                        <div className="font-medium">{axiom.description}</div>
                        <details className="mt-1 text-xs text-muted-foreground">
                          <summary className="cursor-pointer select-none hover:text-foreground">{t("review.showAxiomKey")}</summary>
                          <code className="mt-1 block whitespace-normal break-all rounded bg-muted px-2 py-1.5 text-[11px]">
                            {axiom.axiom_key}
                          </code>
                        </details>
                      </div>
                    ))}
                  </div>
                </div>
                <div className="space-y-2">
                  {evidenceSources.map(({ source, axioms }) => (
                    <div key={source.chunk_id} className="rounded-lg border p-3.5">
                      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
                        <span className="font-medium text-foreground">{source.document ?? t("review.deletedSource")}</span>
                        <span className="text-muted-foreground">{t("review.chunk", { index: source.chunk_index + 1 })}</span>
                        {source.folder && source.folder !== "/" && <span className="text-muted-foreground">{source.folder}</span>}
                      </div>
                      <p className="mt-3 whitespace-pre-wrap break-words text-sm leading-relaxed text-foreground/90">{source.snippet}</p>
                      <p className="mt-3 border-t pt-2 text-[11px] leading-relaxed text-muted-foreground">
                        {t("review.supports", { axioms: axioms.join(" · ") })}
                      </p>
                    </div>
                  ))}
                </div>
              </div>
            ) : (
              <div className="rounded-lg border p-4 text-sm text-muted-foreground">
                {t("review.noSourceEvidence")}
              </div>
            )}
          </section>

          {canWrite ? (
            <div className="flex items-center justify-between gap-3 border-t pt-4">
              <p className="text-xs leading-relaxed text-muted-foreground">
                {t("review.auditNote")}
              </p>
              <Button variant="outline" disabled={busy} onClick={onDismiss}>
                {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <X className="h-3.5 w-3.5" />}
                {t("review.dismissNonIssue")}
              </Button>
            </div>
          ) : (
            <p className="rounded-lg border p-3 text-xs text-muted-foreground">{t("review.readOnly")}</p>
          )}
    </ReviewDetailSheet>
  )
}
