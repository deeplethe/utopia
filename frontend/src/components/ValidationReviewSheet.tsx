import { Check } from "lucide-react"
import type { ValidationFix, Violation } from "@/lib/types"
import { useI18n } from "@/lib/i18n"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ReviewDetailSheet, ReviewStatusBadge } from "@/components/review-bits"

export default function ValidationReviewSheet({
  violation, typeLabel, canWrite, busy, onClose, onFix,
}: {
  violation: Violation | null
  typeLabel: string
  canWrite: boolean
  busy: boolean
  onClose: () => void
  onFix: (fix: ValidationFix) => void
}) {
  const { t } = useI18n()
  if (!violation) return null
  const error = violation.severity === "error"

  return (
    <ReviewDetailSheet
      open
      onOpenChange={(open) => { if (!open && !busy) onClose() }}
      badges={(
        <>
          <ReviewStatusBadge tone={error ? "error" : "warning"} title={error ? t("common.error") : t("common.warning")}>
            {t("common.pending")}
          </ReviewStatusBadge>
          <Badge variant="secondary">{typeLabel}</Badge>
        </>
      )}
      title={violation.individual.label}
      description={violation.summary}
    >
      <section className="space-y-2">
        <h3 className="text-sm font-semibold">{t("review.validation.instanceIri")}</h3>
        <code className="block break-all rounded-lg border bg-muted/40 p-3 text-[11px] leading-relaxed text-muted-foreground">
          {violation.individual.iri}
        </code>
      </section>

      <section className="space-y-3">
        <div>
          <h3 className="text-sm font-semibold">{t("review.validation.availableFixes")}</h3>
          <p className="text-xs text-muted-foreground">{t("review.validation.availableFixesDescription")}</p>
        </div>
        {violation.fixes.length === 0 ? (
          <div className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">{t("review.validation.noFixes")}</div>
        ) : (
          <div className="space-y-2.5">
            {violation.fixes.map((fix) => {
              const fixKind = typeof fix.op.kind === "string" ? fix.op.kind : ""
              const suggestsDeletion = fixKind.startsWith("remove_")
              return (
              <div key={fix.id} className="flex items-start justify-between gap-3 rounded-lg border p-3.5">
                <div className="min-w-0">
                  <p className={`font-medium ${suggestsDeletion ? "text-muted-foreground line-through decoration-1" : ""}`}>
                    {fix.label}
                  </p>
                  <details className="mt-2 text-xs text-muted-foreground">
                    <summary className="cursor-pointer select-none hover:text-foreground">{t("review.technicalOperation")}</summary>
                    <pre className="mt-2 whitespace-pre-wrap break-all rounded-md bg-muted p-2 font-mono text-[11px] leading-relaxed">
                      {JSON.stringify(fix.op, null, 2)}
                    </pre>
                  </details>
                </div>
                {canWrite && (
                  <Button size="sm" className="shrink-0" disabled={busy} onClick={() => onFix(fix)}>
                    <Check className="h-3.5 w-3.5" /> {t("common.apply")}
                  </Button>
                )}
              </div>
              )
            })}
          </div>
        )}
      </section>

      {!canWrite && <p className="rounded-lg border p-3 text-xs text-muted-foreground">{t("review.readOnly")}</p>}
    </ReviewDetailSheet>
  )
}
