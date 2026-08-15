import {
  AlertTriangle,
  ChevronDown,
  FileCode2,
} from "lucide-react"
import type { OntologyChangeSetResult, OntologyImpact } from "@/lib/types"

export type OntologyPreviewData = Partial<OntologyChangeSetResult>

function total(preview: OntologyPreviewData, key: string) {
  const value = preview.impact?.totals?.[key]
  return typeof value === "number" ? value : 0
}

function uniqueIndividuals(
  items: OntologyImpact[] | undefined,
  key: "affected_individuals" | "individuals_deleted" | "individuals_retyped",
) {
  return [...new Set((items ?? []).flatMap((item) => item[key] ?? []))]
}

function shortIri(iri: string) {
  const local = iri.split(/[#/]/).filter(Boolean).pop() ?? iri
  try { return decodeURIComponent(local) } catch { return local }
}

function ImpactList({ title, values, totalCount, moreLabel, warning = false }: {
  title: string
  values: string[]
  totalCount: number
  moreLabel: (count: number) => string
  warning?: boolean
}) {
  if (totalCount === 0) return null
  const visible = values.slice(0, 12)
  const remaining = Math.max(0, totalCount - visible.length)
  return (
    <div className="rounded-md border bg-muted/15 p-2.5">
      <div className={`mb-1.5 text-[11px] font-semibold ${warning ? "text-amber-700 dark:text-amber-300" : "text-muted-foreground"}`}>
        {title} · {totalCount}
      </div>
      {visible.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {visible.map((iri) => (
            <span key={iri} title={iri} className="max-w-40 truncate rounded bg-muted px-1.5 py-0.5 text-[10px]">
              {shortIri(iri)}
            </span>
          ))}
          {remaining > 0 && (
            <span className="px-1 py-0.5 text-[10px] text-muted-foreground">{moreLabel(remaining)}</span>
          )}
        </div>
      )}
    </div>
  )
}

function RdfBlock({ title, added, removed, addedLabel, removedLabel }: {
  title: string
  added?: string
  removed?: string
  addedLabel: string
  removedLabel: string
}) {
  if (!added && !removed) return null
  return (
    <div className="space-y-2">
      <div className="text-[11px] font-semibold text-muted-foreground">{title}</div>
      {added && (
        <div>
          <div className="mb-1 text-[10px] font-medium text-emerald-700 dark:text-emerald-400">{addedLabel}</div>
          <pre className="max-h-36 overflow-auto whitespace-pre-wrap break-all rounded-md bg-emerald-500/5 p-2 font-mono text-[9px] leading-relaxed text-foreground">{added}</pre>
        </div>
      )}
      {removed && (
        <div>
          <div className="mb-1 text-[10px] font-medium text-amber-700 dark:text-amber-300">{removedLabel}</div>
          <pre className="max-h-36 overflow-auto whitespace-pre-wrap break-all rounded-md bg-muted/50 p-2 font-mono text-[9px] leading-relaxed text-foreground">{removed}</pre>
        </div>
      )}
    </div>
  )
}

export default function OntologyImpactPreview({ preview, zh }: {
  preview: OntologyPreviewData
  zh: boolean
}) {
  const copy = {
    details: zh ? "影响明细" : "Impact details",
    structuralBlocked: (value: number) => zh ? `新增 ${value} 个结构错误，当前不能提交` : `${value} new structural error${value === 1 ? "" : "s"}; commit is blocked`,
    tbox: zh ? "TBox · 本体模式" : "TBox · schema",
    abox: zh ? "ABox · 实例数据" : "ABox · instances",
    added: zh ? "新增" : "added",
    removed: zh ? "移除" : "removed",
    impact: zh ? "实例影响" : "Instance impact",
    affected: zh ? "受影响实例" : "Affected individuals",
    deleted: zh ? "将被删除的实例" : "Individuals to be deleted",
    retyped: zh ? "将被重新归类的实例" : "Individuals to be retyped",
    exactDiff: zh ? "查看精确 RDF 差异" : "View exact RDF diff",
    noInstanceImpact: zh ? "没有实例受到这组变更影响" : "No individuals are affected by this change set",
    more: (value: number) => zh ? `另有 ${value} 个` : `+${value} more`,
  }
  const operations = preview.impact?.operations
  const affected = uniqueIndividuals(operations, "affected_individuals")
  const deleted = uniqueIndividuals(operations, "individuals_deleted")
  const retyped = uniqueIndividuals(operations, "individuals_retyped")
  const affectedCount = total(preview, "affected_individuals") || affected.length
  const deletedCount = total(preview, "individuals_deleted") || deleted.length
  const retypedCount = total(preview, "individuals_retyped") || retyped.length
  const validation = preview.structural_validation
  const newStructuralErrors = validation?.new_error_count ?? 0
  const committable = validation?.committable !== false

  return (
    <section className="space-y-3" aria-label={copy.details}>
      <h4 className="text-[11px] font-semibold uppercase tracking-[0.12em] text-muted-foreground">{copy.details}</h4>

      {validation && !committable && (
        <div className="flex items-start gap-2.5 border-b pb-3">
          <div className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-amber-500/10 text-amber-700 dark:text-amber-300">
            <AlertTriangle className="h-3.5 w-3.5" />
          </div>
          <div className="min-w-0 flex-1">
            <p className="text-[11px] font-semibold text-amber-700 dark:text-amber-300">{copy.structuralBlocked(newStructuralErrors)}</p>
            {(validation.new_error_signatures?.length ?? 0) > 0 && (
              <ul className="mt-1.5 list-inside list-disc space-y-0.5 text-[10px] text-amber-700 dark:text-amber-300">
                {validation.new_error_signatures.slice(0, 8).map((signature) => <li key={signature} className="break-all">{signature}</li>)}
              </ul>
            )}
          </div>
        </div>
      )}

      {(affectedCount > 0 || deletedCount > 0 || retypedCount > 0) && (
        <div className="space-y-1.5">
          <div className="text-[11px] font-semibold text-muted-foreground">{copy.impact}</div>
          <div className="grid gap-1.5 sm:grid-cols-2">
            <ImpactList title={copy.affected} values={affected} totalCount={affectedCount} moreLabel={copy.more} />
            <ImpactList title={copy.deleted} values={deleted} totalCount={deletedCount} moreLabel={copy.more} warning />
            <ImpactList title={copy.retyped} values={retyped} totalCount={retypedCount} moreLabel={copy.more} />
          </div>
        </div>
      )}
      {affectedCount === 0 && deletedCount === 0 && retypedCount === 0 && (
        <p className="text-[11px] text-muted-foreground">{copy.noInstanceImpact}</p>
      )}

      {(preview.diff?.tbox_added || preview.diff?.tbox_removed || preview.diff?.abox_added || preview.diff?.abox_removed) && (
        <details className="rounded-md border bg-background">
          <summary className="flex cursor-pointer list-none items-center gap-1.5 px-2.5 py-2 text-[11px] font-semibold text-muted-foreground hover:text-foreground">
            <FileCode2 className="h-3.5 w-3.5" /> {copy.exactDiff}
            <ChevronDown className="ml-auto h-3.5 w-3.5" />
          </summary>
          <div className="space-y-3 border-t p-2.5">
            <RdfBlock title={copy.tbox} added={preview.diff.tbox_added} removed={preview.diff.tbox_removed} addedLabel={copy.added} removedLabel={copy.removed} />
            <RdfBlock title={copy.abox} added={preview.diff.abox_added} removed={preview.diff.abox_removed} addedLabel={copy.added} removedLabel={copy.removed} />
          </div>
        </details>
      )}
    </section>
  )
}
