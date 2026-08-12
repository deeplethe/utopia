import { useEffect, useMemo, useState, type ReactNode } from "react"
import { Link } from "react-router-dom"
import {
  AlertTriangle, ArrowRight, BookOpenText, Boxes, CheckCircle2, Clock3, Database,
  FileText, GitBranch, Languages, Network, ShieldCheck, Sparkles,
} from "lucide-react"
import { api } from "@/lib/api"
import type {
  Conflict, ExtractionJob, KnowledgeSystem, OntologyView, ReviewCounts, SourceDoc,
  VocabularyStats,
} from "@/lib/types"
import { useI18n } from "@/lib/i18n"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"

function MetricCard({ icon, label, value, detail }: {
  icon: ReactNode
  label: string
  value: ReactNode
  detail: ReactNode
}) {
  return (
    <Card className="overflow-hidden">
      <CardContent className="p-4">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs font-medium text-muted-foreground">{label}</p>
            <p className="mt-1 text-2xl font-semibold tabular-nums tracking-tight">{value}</p>
          </div>
          <div className="rounded-lg bg-muted p-2 text-muted-foreground">{icon}</div>
        </div>
        <p className="mt-3 truncate text-[11px] text-muted-foreground" title={typeof detail === "string" ? detail : undefined}>
          {detail}
        </p>
      </CardContent>
    </Card>
  )
}

function CoverageRow({ label, detail, value, total }: {
  label: string
  detail: string
  value: number
  total: number
}) {
  const percentage = total > 0 ? Math.round((value / total) * 100) : 0
  return (
    <div className="space-y-2">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <p className="text-sm font-medium">{label}</p>
          <p className="mt-0.5 text-xs text-muted-foreground">{detail}</p>
        </div>
        <span className="shrink-0 text-sm font-semibold tabular-nums">{percentage}%</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-muted">
        <div className="h-full rounded-full bg-primary transition-all" style={{ width: `${percentage}%` }} />
      </div>
    </div>
  )
}

function QueueRow({ href, icon, label, count, detail }: {
  href: string
  icon: ReactNode
  label: string
  count: number | null
  detail?: string
}) {
  return (
    <Link
      to={href}
      className="group flex items-center gap-3 rounded-lg border border-transparent px-3 py-2.5 transition-colors hover:border-border hover:bg-muted/40"
    >
      <span className="text-muted-foreground">{icon}</span>
      <span className="min-w-0 flex-1">
        <span className="block text-sm font-medium">{label}</span>
        {detail && <span className="block truncate text-[11px] text-muted-foreground">{detail}</span>}
      </span>
      <Badge variant={count && count > 0 ? "secondary" : "outline"} className="min-w-7 justify-center tabular-nums">
        {count ?? "—"}
      </Badge>
      <ArrowRight className="h-3.5 w-3.5 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
    </Link>
  )
}

export default function KsOverview({ ks, view, sources, conflicts, jobs }: {
  ks: KnowledgeSystem
  view: OntologyView
  sources: SourceDoc[]
  conflicts: Conflict[]
  jobs: ExtractionJob[]
}) {
  const { locale, t } = useI18n()
  const [reviewCounts, setReviewCounts] = useState<ReviewCounts | null>(null)
  const [vocabularyStats, setVocabularyStats] = useState<VocabularyStats | null>(null)
  const [instanceSummary, setInstanceSummary] = useState<{ total: number; types: number } | null>(null)

  useEffect(() => {
    let cancelled = false
    const load = async () => {
      const [review, vocabulary, instances] = await Promise.allSettled([
        api.reviewCounts(ks.id),
        api.listVocabularySchemes(ks.id),
        api.aboxClasses(ks.id),
      ])
      if (cancelled) return
      if (review.status === "fulfilled") setReviewCounts(review.value)
      if (vocabulary.status === "fulfilled") setVocabularyStats(vocabulary.value.stats)
      if (instances.status === "fulfilled") {
        setInstanceSummary({
          total: instances.value.total,
          types: instances.value.classes.filter((item) => item.count > 0).length,
        })
      }
    }
    load()
    return () => { cancelled = true }
  }, [ks.id, ks.updated_at])

  const properties = useMemo(
    () => [...view.object_properties, ...view.data_properties],
    [view.data_properties, view.object_properties],
  )
  const entityCount = view.classes.length + properties.length
  const rootCount = view.classes.filter((item) => item.superclasses.length === 0).length
  const hierarchyLinked = view.classes.filter((item) => item.superclasses.length > 0).length
  const constrainedProperties = properties.filter((item) => item.domain && item.range).length
  const documentedEntities = [...view.classes, ...properties].filter((item) => item.comment.trim()).length
  const otherAxioms = view.axioms.disjoint_with.length + view.axioms.equivalent_class.length
  const errorConflicts = conflicts.filter((item) => item.severity === "error").length
  const warningConflicts = conflicts.length - errorConflicts
  const latestJob = useMemo(
    () => [...jobs]
      .filter((job) => job.status === "completed" || job.status === "failed")
      .sort((left, right) => new Date(right.finished_at ?? right.created_at).getTime()
        - new Date(left.finished_at ?? left.created_at).getTime())[0] ?? null,
    [jobs],
  )
  const totalChunks = sources.reduce((sum, source) => sum + source.chunk_count, 0)
  const totalEvidenceLinks = sources.reduce((sum, source) => sum + source.axiom_count, 0)
  const topSources = useMemo(
    () => [...sources].sort((left, right) => right.axiom_count - left.axiom_count).slice(0, 5),
    [sources],
  )
  const nextQueue = reviewCounts
    ? ([
        ["conflicts", reviewCounts.conflicts],
        ["resolution", reviewCounts.resolution],
        ["terminology", reviewCounts.terminology],
        ["validation", reviewCounts.validation],
      ] as const).find(([, count]) => count > 0)?.[0]
    : undefined
  const latestStatus = latestJob
    ? t(latestJob.status === "completed" ? "extractionQueue.status.completed" : "extractionQueue.status.failed")
    : ""

  return (
    <div className="space-y-5">
      <section>
        <div className="mb-3 flex items-center gap-2">
          <Boxes className="h-4 w-4 text-muted-foreground" />
          <h2 className="text-sm font-semibold">{t("overview.knowledgeSnapshot")}</h2>
        </div>
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-5">
          <MetricCard
            icon={<GitBranch className="h-4 w-4" />}
            label={t("common.classes")}
            value={view.stats.class_count}
            detail={t("overview.classesDetail", { count: rootCount })}
          />
          <MetricCard
            icon={<Network className="h-4 w-4" />}
            label={t("common.properties")}
            value={view.stats.property_count}
            detail={t("overview.propertiesDetail", {
              object: view.object_properties.length,
              data: view.data_properties.length,
            })}
          />
          <MetricCard
            icon={<BookOpenText className="h-4 w-4" />}
            label={t("common.axioms")}
            value={view.stats.axiom_count}
            detail={t("overview.axiomsDetail", {
              subclass: view.axioms.subclass_of.length,
              constraints: otherAxioms,
            })}
          />
          <MetricCard
            icon={<Languages className="h-4 w-4" />}
            label={t("overview.concepts")}
            value={vocabularyStats?.concept_count ?? "—"}
            detail={vocabularyStats
              ? t("overview.conceptsDetail", {
                  labels: vocabularyStats.label_count,
                  mapped: vocabularyStats.mapped_count,
                })
              : t("common.loading")}
          />
          <MetricCard
            icon={<Database className="h-4 w-4" />}
            label={t("overview.instances")}
            value={instanceSummary?.total ?? "—"}
            detail={instanceSummary
              ? t("overview.instancesDetail", { count: instanceSummary.types })
              : t("common.loading")}
          />
        </div>
      </section>

      <div className="grid gap-4 xl:grid-cols-12">
        <Card className="xl:col-span-7">
          <CardHeader className="flex flex-row items-start justify-between gap-4 pb-2">
            <div>
              <CardTitle className="flex items-center gap-2 text-sm">
                {reviewCounts?.total === 0
                  ? <ShieldCheck className="h-4 w-4 text-emerald-600" />
                  : <AlertTriangle className="h-4 w-4 text-amber-600" />}
                {t("overview.governance")}
              </CardTitle>
              <p className="mt-1 text-xs text-muted-foreground">
                {reviewCounts
                  ? reviewCounts.total === 0
                    ? t("overview.governanceClear")
                    : t("overview.governancePending", { count: reviewCounts.total })
                  : t("common.loading")}
              </p>
            </div>
            {reviewCounts && (
              <Badge variant={reviewCounts.total === 0 ? "outline" : "secondary"} className="tabular-nums">
                {reviewCounts.total === 0 ? t("overview.clear") : reviewCounts.total}
              </Badge>
            )}
          </CardHeader>
          <CardContent className="grid gap-1 p-3 pt-0 sm:grid-cols-2">
            <QueueRow
              href={`/knowledge/${ks.id}/review/conflicts`}
              icon={<AlertTriangle className="h-4 w-4" />}
              label={t("sidebar.conflicts")}
              count={reviewCounts?.conflicts ?? null}
              detail={t("overview.conflictBreakdown", { errors: errorConflicts, warnings: warningConflicts })}
            />
            <QueueRow
              href={`/knowledge/${ks.id}/review/resolution`}
              icon={<Sparkles className="h-4 w-4" />}
              label={t("sidebar.entityResolution")}
              count={reviewCounts?.resolution ?? null}
            />
            <QueueRow
              href={`/knowledge/${ks.id}/review/terminology`}
              icon={<Languages className="h-4 w-4" />}
              label={t("sidebar.terminology")}
              count={reviewCounts?.terminology ?? null}
            />
            <QueueRow
              href={`/knowledge/${ks.id}/review/validation`}
              icon={<CheckCircle2 className="h-4 w-4" />}
              label={t("sidebar.validation")}
              count={reviewCounts?.validation ?? null}
            />
            <div className="sm:col-span-2 flex justify-end px-3 pt-1">
              <Button asChild size="sm" variant="ghost" className="h-7 gap-1 text-xs">
                <Link to={nextQueue
                  ? `/knowledge/${ks.id}/review/${nextQueue}`
                  : `/knowledge/${ks.id}/releases`}>
                  {nextQueue ? t("overview.openReview") : t("overview.openReleases")}
                  <ArrowRight className="h-3.5 w-3.5" />
                </Link>
              </Button>
            </div>
          </CardContent>
        </Card>

        <Card className="xl:col-span-5">
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-sm">
              <Clock3 className="h-4 w-4 text-muted-foreground" /> {t("overview.latestExtraction")}
            </CardTitle>
          </CardHeader>
          <CardContent>
            {!latestJob ? (
              <div className="flex min-h-36 flex-col items-center justify-center gap-3 rounded-lg border border-dashed text-center">
                <Sparkles className="h-5 w-5 text-muted-foreground" />
                <p className="text-sm text-muted-foreground">{t("overview.noExtraction")}</p>
                <Button asChild size="sm" variant="outline">
                  <Link to={`/knowledge/${ks.id}/documents`}>{t("overview.openDocuments")}</Link>
                </Button>
              </div>
            ) : (
              <div className="space-y-4">
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <p className="text-sm font-medium">{t(`extract.mode.${latestJob.kind}`)}</p>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {new Date(latestJob.finished_at ?? latestJob.created_at).toLocaleString(locale)}
                    </p>
                  </div>
                  <Badge variant={latestJob.status === "failed" ? "destructive" : "outline"}>{latestStatus}</Badge>
                </div>
                <div className="grid grid-cols-2 gap-3 rounded-lg bg-muted/40 p-3 text-xs">
                  <div>
                    <p className="text-muted-foreground">{t("extract.model")}</p>
                    <p className="mt-1 truncate font-medium" title={latestJob.model}>{latestJob.model}</p>
                  </div>
                  <div>
                    <p className="text-muted-foreground">{t("overview.processedEvidence")}</p>
                    <p className="mt-1 font-medium tabular-nums">
                      {t("overview.chunksProcessed", { count: latestJob.processed_chunks })}
                    </p>
                  </div>
                </div>
                {latestJob.status === "failed" ? (
                  <p className="line-clamp-3 text-xs leading-relaxed text-destructive">{latestJob.error || t("overview.extractionFailed")}</p>
                ) : (
                  <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
                    {(latestJob.kind === "tbox" || latestJob.kind === "both") && (
                      <Badge variant="secondary">{t("overview.schemaResult", {
                        classes: latestJob.classes_added,
                        properties: latestJob.properties_added,
                        axioms: latestJob.axioms_added,
                      })}</Badge>
                    )}
                    {(latestJob.kind === "abox" || latestJob.kind === "both") && (
                      <Badge variant="secondary">{t("overview.instanceResult", {
                        instances: latestJob.individuals_added,
                        assertions: latestJob.assertions_added,
                      })}</Badge>
                    )}
                    <Badge variant="secondary">{t("overview.terminologyResult", {
                      terms: latestJob.terms_added,
                      proposals: latestJob.terminology_proposals,
                    })}</Badge>
                  </div>
                )}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      <div className="grid gap-4 xl:grid-cols-12">
        <Card className="xl:col-span-7">
          <CardHeader className="flex flex-row items-center justify-between gap-4 pb-3">
            <CardTitle className="flex items-center gap-2 text-sm">
              <Network className="h-4 w-4 text-muted-foreground" /> {t("overview.structureCoverage")}
            </CardTitle>
            <Button asChild variant="ghost" size="sm" className="h-7 gap-1 text-xs">
              <Link to={`/knowledge/${ks.id}/ontology`}>
                {t("overview.openOntology")} <ArrowRight className="h-3.5 w-3.5" />
              </Link>
            </Button>
          </CardHeader>
          <CardContent className="space-y-5">
            <CoverageRow
              label={t("overview.hierarchyCoverage")}
              detail={t("overview.hierarchyCoverageDetail", { linked: hierarchyLinked, total: view.classes.length })}
              value={hierarchyLinked}
              total={view.classes.length}
            />
            <CoverageRow
              label={t("overview.propertyCoverage")}
              detail={t("overview.propertyCoverageDetail", { constrained: constrainedProperties, total: properties.length })}
              value={constrainedProperties}
              total={properties.length}
            />
            <CoverageRow
              label={t("overview.documentationCoverage")}
              detail={t("overview.documentationCoverageDetail", { documented: documentedEntities, total: entityCount })}
              value={documentedEntities}
              total={entityCount}
            />
          </CardContent>
        </Card>

        <Card className="xl:col-span-5">
          <CardHeader className="flex flex-row items-start justify-between gap-4 pb-3">
            <div>
              <CardTitle className="flex items-center gap-2 text-sm">
                <FileText className="h-4 w-4 text-muted-foreground" /> {t("overview.evidenceCoverage")}
              </CardTitle>
              <p className="mt-1 text-xs text-muted-foreground">
                {t("overview.evidenceSummary", {
                  documents: sources.length,
                  chunks: totalChunks,
                  links: totalEvidenceLinks,
                })}
              </p>
            </div>
            <Button asChild variant="ghost" size="sm" className="h-7 shrink-0 gap-1 text-xs">
              <Link to={`/knowledge/${ks.id}/documents`}>
                {t("overview.openDocuments")} <ArrowRight className="h-3.5 w-3.5" />
              </Link>
            </Button>
          </CardHeader>
          <CardContent>
            {topSources.length === 0 ? (
              <div className="flex h-32 items-center justify-center rounded-lg border border-dashed text-sm text-muted-foreground">
                {t("overview.noEvidence")}
              </div>
            ) : (
              <div className="space-y-3">
                {topSources.map((source) => {
                  const percentage = totalEvidenceLinks > 0
                    ? Math.round((source.axiom_count / totalEvidenceLinks) * 100)
                    : 0
                  return (
                    <div key={source.document_id} className="space-y-1.5">
                      <div className="flex items-center justify-between gap-3 text-xs">
                        <span className={`min-w-0 truncate font-medium ${source.exists ? "" : "line-through text-muted-foreground"}`} title={source.filename}>
                          {source.filename}
                        </span>
                        <span className="shrink-0 tabular-nums text-muted-foreground">
                          {t("overview.evidenceLinks", { count: source.axiom_count })}
                        </span>
                      </div>
                      <div className="h-1.5 overflow-hidden rounded-full bg-muted">
                        <div className="h-full rounded-full bg-chart-2" style={{ width: `${percentage}%` }} />
                      </div>
                    </div>
                  )
                })}
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
