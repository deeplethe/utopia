import { useCallback, useEffect, useMemo, useState } from "react"
import { Link } from "react-router-dom"
import {
  AlertCircle, ArrowRight, Check, ChevronDown, ChevronUp, Copy, Download, Ellipsis,
  GitCompare, Loader2, PackageCheck, Plus, RotateCcw, Server,
  ServerOff, Trash2,
} from "lucide-react"
import { toast } from "sonner"
import { api, ApiError } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type {
  ExportJob, OntologyRelease, ReleaseDiff, ReleaseLayer, ReleaseQualityGate,
} from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"

function statementCount(release: OntologyRelease, layer: "tbox" | "vocabulary" | "abox") {
  return release.manifest.layers?.[layer]?.statements ?? 0
}

function totalStatements(release: OntologyRelease) {
  return statementCount(release, "tbox")
    + statementCount(release, "vocabulary")
    + statementCount(release, "abox")
}

function qualityGateFromError(error: unknown): ReleaseQualityGate | null {
  if (!(error instanceof ApiError) || !error.detail || typeof error.detail !== "object") return null
  const detail = error.detail as Record<string, unknown>
  if (detail.message !== "Release quality gate failed"
    || !detail.quality_gate || typeof detail.quality_gate !== "object") return null
  const gate = detail.quality_gate as Record<string, unknown>
  const fields = [
    "open_conflict_errors", "unresolved_entities", "pending_terminology", "validation_errors", "blocking",
  ] as const
  if (!fields.every((field) => typeof gate[field] === "number")) return null
  return {
    open_conflict_errors: gate.open_conflict_errors as number,
    unresolved_entities: gate.unresolved_entities as number,
    pending_terminology: gate.pending_terminology as number,
    validation_errors: gate.validation_errors as number,
    blocking: gate.blocking as number,
  }
}

type ToolTab = "versions" | "diff" | "exports"

export default function ReleasePanel({
  ksId, canWrite, canManage, onChanged,
}: {
  ksId: number
  canWrite: boolean
  canManage: boolean
  onChanged?: () => void
}) {
  const { locale, t } = useI18n()
  const confirmAction = useConfirm()
  const releaseLabel = (release: OntologyRelease) => (
    release.published_at ? release.version : t("release.draftNumber", { id: release.id })
  )
  const [releases, setReleases] = useState<OntologyRelease[]>([])
  const [exports, setExports] = useState<ExportJob[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<string | null>(null)
  const [toolTab, setToolTab] = useState<ToolTab>("versions")
  const [expandedReleaseId, setExpandedReleaseId] = useState<number | null>(null)
  const [expandedExportId, setExpandedExportId] = useState<number | null>(null)
  const [fromId, setFromId] = useState("")
  const [toId, setToId] = useState("")
  const [exportSource, setExportSource] = useState("")
  const [diff, setDiff] = useState<ReleaseDiff | null>(null)
  const [qualityGateFailure, setQualityGateFailure] = useState<{
    releaseId: number
    gate: ReleaseQualityGate
  } | null>(null)

  const load = useCallback(async (quiet = false) => {
    if (!quiet) setLoading(true)
    try {
      const [releaseList, exportList] = await Promise.all([api.listReleases(ksId), api.listExports(ksId)])
      setReleases(releaseList.items)
      setExports(exportList.items)
    } catch (error) {
      if (!quiet) toast.error(t("release.loadFailed", { error: (error as Error).message }))
    } finally {
      if (!quiet) setLoading(false)
    }
  }, [ksId, t])

  useEffect(() => { load() }, [load])

  const hasRunning = releases.some((item) => ["pending", "running"].includes(item.manifest.capture_status))
    || releases.some((item) => ["provisioning", "stopping"].includes(item.deployment?.status ?? ""))
    || exports.some((item) => item.status === "pending" || item.status === "running")

  useEffect(() => {
    if (!hasRunning) return
    const timer = window.setInterval(() => load(true), 1500)
    return () => window.clearInterval(timer)
  }, [hasRunning, load])

  const readyReleases = useMemo(
    () => releases.filter((item) => item.manifest.capture_status === "ready" && item.status !== "deleted"),
    [releases],
  )
  const currentRelease = useMemo(
    () => releases.find((item) => item.status !== "deleted") ?? null,
    [releases],
  )
  const historicalReleases = useMemo(
    () => releases.filter((item) => item.id !== currentRelease?.id),
    [releases, currentRelease],
  )

  useEffect(() => {
    if (!fromId && readyReleases[1]) setFromId(String(readyReleases[1].id))
    if (!toId && readyReleases[0]) setToId(String(readyReleases[0].id))
    if (!exportSource && !loading) {
      setExportSource(readyReleases[0] ? String(readyReleases[0].id) : "workspace")
    }
  }, [readyReleases, fromId, toId, exportSource, loading])

  const perform = async (key: string, action: () => Promise<unknown>, success: string) => {
    setBusy(key)
    try {
      await action()
      toast.success(success)
      await load(true)
      onChanged?.()
    } catch (error) {
      toast.error((error as Error).message.replace(/^\d+:\s*/, ""))
    } finally {
      setBusy(null)
    }
  }

  const createDraft = () => {
    setQualityGateFailure(null)
    return perform("create", () => api.createRelease(ksId), t("release.draftCreated"))
  }

  const reviewRelease = async (release: OntologyRelease) => {
    const key = `review-${release.id}`
    setBusy(key)
    setQualityGateFailure(null)
    try {
      await api.reviewRelease(ksId, release.id)
      toast.success(t("release.reviewed"))
      await load(true)
      onChanged?.()
    } catch (error) {
      const gate = qualityGateFromError(error)
      if (gate) {
        setQualityGateFailure({ releaseId: release.id, gate })
        toast.error(t("release.qualityGateFailed"))
      } else {
        toast.error((error as Error).message.replace(/^\d+:\s*/, ""))
      }
    } finally {
      setBusy(null)
    }
  }

  const startExport = (layer: ReleaseLayer) => {
    const releaseId = exportSource === "workspace" ? undefined : Number(exportSource)
    return perform(
      `export-${layer}-${exportSource}`,
      () => api.createExport(ksId, layer, releaseId),
      t("release.exportStarted"),
    )
  }

  const runDiff = async () => {
    if (!fromId || !toId || fromId === toId) return
    setBusy("diff")
    try {
      setDiff(await api.diffReleases(ksId, Number(fromId), Number(toId)))
    } catch (error) {
      toast.error((error as Error).message.replace(/^\d+:\s*/, ""))
    } finally {
      setBusy(null)
    }
  }

  const copyEndpoint = async (release: OntologyRelease) => {
    if (!release.service_url) return
    await navigator.clipboard.writeText(`${window.location.origin}${release.service_url}`)
    toast.success(t("release.endpointCopied"))
  }

  const stopService = async (release: OntologyRelease) => {
    if (await confirmAction(t("release.stopServiceConfirm", { version: releaseLabel(release) }), { destructive: true })) {
      await perform(`stop-${release.id}`, () => api.stopReleaseService(ksId, release.id), t("release.serviceStopping"))
    }
  }

  const rollback = async (release: OntologyRelease) => {
    if (await confirmAction(t("release.rollbackConfirm", { version: releaseLabel(release) }), { destructive: true })) {
      await perform(`rollback-${release.id}`, () => api.rollbackRelease(ksId, release.id), t("release.rolledBack"))
    }
  }

  const deleteRelease = async (release: OntologyRelease) => {
    if (await confirmAction(
      t("release.deleteConfirm", { version: releaseLabel(release) }),
      { destructive: true, confirmLabel: t("release.delete") },
    )) {
      await perform(`delete-${release.id}`, () => api.deleteRelease(ksId, release.id), t("release.releaseDeleted"))
    }
  }

  const openExportCenter = (release: OntologyRelease) => {
    setExportSource(String(release.id))
    setToolTab("exports")
  }

  const statusBadge = (release: OntologyRelease) => (
    <Badge variant={release.status === "published" ? "default" : release.status === "deleted" ? "destructive" : "secondary"}>
      {t(`release.status.${release.status}`)}
    </Badge>
  )

  const serviceLabel = (release: OntologyRelease) => {
    const status = release.deployment?.status ?? "stopped"
    if (status === "active") return t("release.serviceActive")
    if (status === "provisioning") return t("release.service.provisioning")
    if (status === "stopping") return t("release.service.stopping")
    if (status === "failed") return t("release.serviceFailed")
    return t("release.serviceStopped")
  }

  const qualityGateAlert = (release: OntologyRelease) => {
    if (qualityGateFailure?.releaseId !== release.id) return null
    const { gate } = qualityGateFailure
    const issues = [
      {
        count: gate.open_conflict_errors,
        label: t("release.qualityGate.conflicts"),
        href: `/knowledge/${ksId}/review/conflicts`,
      },
      {
        count: gate.unresolved_entities,
        label: t("release.qualityGate.resolution"),
        href: `/knowledge/${ksId}/review/resolution`,
      },
      {
        count: gate.pending_terminology,
        label: t("release.qualityGate.terminology"),
        href: `/knowledge/${ksId}/review/terminology`,
      },
      {
        count: gate.validation_errors,
        label: t("release.qualityGate.validation"),
        href: `/knowledge/${ksId}/review/validation`,
      },
    ].filter((issue) => issue.count > 0)
    return (
      <div role="alert" className="flex gap-3 rounded-lg border border-amber-500/30 bg-amber-500/5 p-3">
        <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium">{t("release.qualityGateFailed")}</p>
          <p className="mt-1 text-xs text-muted-foreground">
            {t("release.qualityGateDescription", { count: gate.blocking })}
          </p>
          <div className="mt-3 flex flex-wrap gap-2">
            {issues.map((issue) => (
              <Button key={issue.href} asChild size="sm" variant="outline" className="bg-background/80">
                <Link to={issue.href}>
                  {issue.label}
                  <Badge variant="secondary" className="ml-1 tabular-nums">{issue.count}</Badge>
                  <ArrowRight className="h-3.5 w-3.5" />
                </Link>
              </Button>
            ))}
          </div>
        </div>
      </div>
    )
  }

  const primaryAction = (release: OntologyRelease) => {
    const capture = release.manifest.capture_status
    if (capture === "pending" || capture === "running") {
      return <Button size="sm" disabled><Loader2 className="animate-spin" />{t("release.capturing")}</Button>
    }
    if (!canWrite || release.status === "deleted" || capture === "failed") return null
    if (release.status === "draft" && capture === "ready") {
      return (
        <Button size="sm" disabled={busy !== null} onClick={() => { void reviewRelease(release) }}>
          {busy === `review-${release.id}` ? <Loader2 className="animate-spin" /> : <Check />}
          {t("release.review")}
        </Button>
      )
    }
    if (release.status === "reviewed") {
      return (
        <Button size="sm" disabled={busy !== null} onClick={() => perform(
          `publish-${release.id}`, () => api.publishRelease(ksId, release.id), t("release.published"),
        )}>
          {busy === `publish-${release.id}` ? <Loader2 className="animate-spin" /> : <PackageCheck />}
          {t("release.publish")}
        </Button>
      )
    }
    if (release.status === "published" && release.deployment?.status === "active" && release.service_url) {
      return <Button size="sm" onClick={() => copyEndpoint(release)}><Copy />{t("release.copyEndpoint")}</Button>
    }
    if (release.status === "published" && ["provisioning", "stopping"].includes(release.deployment?.status ?? "")) {
      return <Button size="sm" disabled><Loader2 className="animate-spin" />{serviceLabel(release)}</Button>
    }
    if (release.status === "published") {
      return (
        <Button size="sm" disabled={busy !== null} onClick={() => perform(
          `deploy-${release.id}`, () => api.deployRelease(ksId, release.id), t("release.deployStarted"),
        )}>
          {busy === `deploy-${release.id}` ? <Loader2 className="animate-spin" /> : <Server />}
          {t("release.deploy")}
        </Button>
      )
    }
    return null
  }

  const actionMenu = (release: OntologyRelease) => {
    if (release.status === "deleted") return null
    return (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button size="icon-sm" variant="outline" disabled={busy !== null} title={t("release.moreActions")}>
            <Ellipsis />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          {release.manifest.capture_status === "ready" && (
            <DropdownMenuItem onSelect={() => openExportCenter(release)}>
              <Download />{t("release.exportThisVersion")}
            </DropdownMenuItem>
          )}
          {canWrite && release.deployment?.status === "active" && (
            <DropdownMenuItem onSelect={() => { void stopService(release) }}>
              <ServerOff />{t("release.stopService")}
            </DropdownMenuItem>
          )}
          {canWrite && release.manifest.capture_status === "ready" && (
            <DropdownMenuItem onSelect={() => { void rollback(release) }}>
              <RotateCcw />{t("release.rollback")}
            </DropdownMenuItem>
          )}
          {canManage && <DropdownMenuSeparator />}
          {canManage && (
            <DropdownMenuItem className="text-destructive focus:text-destructive" onSelect={() => { void deleteRelease(release) }}>
              <Trash2 />{t("release.delete")}
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    )
  }

  const workflow = (release: OntologyRelease) => {
    const captureReady = release.manifest.capture_status === "ready"
    const reviewed = release.status === "reviewed" || release.status === "published"
    const published = release.status === "published"
    const serving = release.deployment?.status === "active"
    const stages = [
      { label: t("release.stage.snapshot"), done: captureReady, active: !captureReady },
      { label: t("release.stage.review"), done: reviewed, active: captureReady && release.status === "draft" },
      { label: t("release.stage.publish"), done: published, active: release.status === "reviewed" },
      { label: t("release.stage.service"), done: serving, active: published && !serving },
    ]
    return (
      <div className="flex flex-col rounded-lg bg-muted/40 px-3 py-3 md:flex-row md:items-center">
        {stages.map((stage, index) => {
          const next = stages[index + 1]
          const connector = next?.done
            ? "bg-emerald-500"
            : stage.done && next?.active ? "bg-primary/60" : "bg-border"
          return (
          <div
            key={stage.label}
            className={`relative flex min-w-0 items-center pb-4 last:pb-0 md:pb-0 ${
              index < stages.length - 1 ? "md:flex-1" : "md:flex-none"
            } ${
              stage.done ? "text-foreground" : stage.active ? "text-foreground" : "text-muted-foreground"
            }`}
          >
            <span className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold ${
              stage.done ? "bg-emerald-500 text-white" : stage.active ? "bg-primary text-primary-foreground" : "bg-muted"
            }`}>
              {stage.done ? <Check className="h-3 w-3" /> : index + 1}
            </span>
            <span className="ml-2 whitespace-nowrap text-xs font-medium">{stage.label}</span>
            {next && (
              <span
                aria-hidden="true"
                className={`absolute left-[9.5px] top-5 h-4 w-px md:static md:mx-3 md:h-px md:min-w-4 md:flex-1 md:w-auto ${connector}`}
              />
            )}
          </div>
          )
        })}
      </div>
    )
  }

  const layerStats = (release: OntologyRelease) => (
    <div className="grid grid-cols-3 rounded-lg bg-muted/40">
      {(["tbox", "vocabulary", "abox"] as const).map((layer) => (
        <div key={layer} className="min-w-0 px-3 py-2 text-center">
          <p className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">{t(`release.layer.${layer}`)}</p>
          <p className="mt-0.5 font-mono text-sm font-medium tabular-nums">{statementCount(release, layer).toLocaleString()}</p>
        </div>
      ))}
    </div>
  )

  if (loading) return <p className="text-sm text-muted-foreground">{t("common.loading")}</p>

  return (
    <div className="w-full min-w-0 space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-2xl">
          <h2 className="flex items-center gap-2 text-sm font-semibold">
            <PackageCheck className="h-4 w-4 text-muted-foreground" /> {t("release.title")}
          </h2>
          <p className="mt-1 text-xs text-muted-foreground">{t("release.description")}</p>
        </div>
        {canWrite && releases.length > 0 && (
          <Button size="sm" variant="outline" onClick={createDraft} disabled={busy !== null}>
            {busy === "create" ? <Loader2 className="animate-spin" /> : <Plus />}
            {t("release.createDraft")}
          </Button>
        )}
      </div>

      {currentRelease ? (
        <Card size="sm">
          <CardHeader className="pb-2">
            <div className="flex flex-wrap items-center gap-2">
              <CardTitle className="flex items-center gap-2 text-sm">
                <PackageCheck className="h-4 w-4 text-muted-foreground" />
                {t("release.currentVersion")} · {releaseLabel(currentRelease)}
              </CardTitle>
              {statusBadge(currentRelease)}
              {currentRelease.status === "published" && (
                <Badge variant="outline" className="gap-1"><Server className="h-3 w-3" />{serviceLabel(currentRelease)}</Badge>
              )}
            </div>
            <CardDescription className="text-xs">
              {totalStatements(currentRelease).toLocaleString()} {t("release.statementsTotal")} · {new Date(currentRelease.created_at).toLocaleString(locale)}
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {workflow(currentRelease)}
            {qualityGateAlert(currentRelease)}
            {currentRelease.manifest.capture_status === "failed" && (
              <div className="flex gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
                <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
                <span>{currentRelease.manifest.error || t("release.captureFailed")}</span>
              </div>
            )}
            <div className="grid gap-3 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-end">
              {layerStats(currentRelease)}
              <div className="flex justify-end gap-2">
                {primaryAction(currentRelease)}
                {actionMenu(currentRelease)}
              </div>
            </div>
          </CardContent>
        </Card>
      ) : (
        <Card size="sm" className="border-dashed py-8 text-center">
          <CardContent className="space-y-3">
            <PackageCheck className="mx-auto h-8 w-8 text-muted-foreground" />
            <div>
              <p className="font-medium">{t("release.empty")}</p>
              <p className="mt-1 text-sm text-muted-foreground">{t("release.description")}</p>
            </div>
            {canWrite && (
              <Button onClick={createDraft} disabled={busy !== null}>
                {busy === "create" ? <Loader2 className="animate-spin" /> : <Plus />}
                {t("release.createDraft")}
              </Button>
            )}
          </CardContent>
        </Card>
      )}

      <Tabs value={toolTab} onValueChange={(value) => setToolTab(value as ToolTab)} className="min-w-0 gap-3">
        <TabsList>
          <TabsTrigger value="versions">{t("release.versionHistory")}</TabsTrigger>
          <TabsTrigger value="diff">{t("release.compareVersions")}</TabsTrigger>
          <TabsTrigger value="exports">{t("release.exportCenter")}</TabsTrigger>
        </TabsList>

        <TabsContent value="versions" className="space-y-3">
          {historicalReleases.length === 0 ? (
            <div className="rounded-lg border border-dashed px-4 py-8 text-center text-sm text-muted-foreground">
              {t("release.noHistory")}
            </div>
          ) : historicalReleases.map((release) => {
            const expanded = expandedReleaseId === release.id
            return (
              <div key={release.id} className="rounded-lg border bg-background">
                <div className="flex flex-wrap items-center justify-between gap-3 p-3">
                  <div className="min-w-0 space-y-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-semibold">{releaseLabel(release)}</span>
                      {statusBadge(release)}
                      {release.status === "published" && <Badge variant="outline">{serviceLabel(release)}</Badge>}
                    </div>
                    <p className="text-xs text-muted-foreground">
                      {totalStatements(release).toLocaleString()} {t("release.statementsTotal")} · {new Date(release.created_at).toLocaleString(locale)}
                    </p>
                  </div>
                  <div className="flex items-center gap-2">
                    {primaryAction(release)}
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => setExpandedReleaseId(expanded ? null : release.id)}
                    >
                      {expanded ? <ChevronUp /> : <ChevronDown />}
                      {expanded ? t("release.hideDetails") : t("release.details")}
                    </Button>
                    {actionMenu(release)}
                  </div>
                </div>
                {qualityGateFailure?.releaseId === release.id && (
                  <div className="border-t p-3">{qualityGateAlert(release)}</div>
                )}
                {expanded && (
                  <div className="grid gap-3 border-t bg-muted/15 p-3 md:grid-cols-[minmax(0,1fr)_minmax(16rem,0.7fr)]">
                    {layerStats(release)}
                    <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs">
                      <dt className="text-muted-foreground">{t("release.snapshot")}</dt>
                      <dd>{release.manifest.capture_status === "ready" ? t("release.ready") : t(`release.capture.${release.manifest.capture_status}`)}</dd>
                      <dt className="text-muted-foreground">{t("release.service")}</dt>
                      <dd>{serviceLabel(release)}</dd>
                      <dt className="text-muted-foreground">{t("release.createdBy")}</dt>
                      <dd>{release.created_by || "—"}</dd>
                      <dt className="text-muted-foreground">{t("release.reviewedBy")}</dt>
                      <dd>{release.reviewed_by || "—"}</dd>
                      <dt className="text-muted-foreground">{t("release.publishedBy")}</dt>
                      <dd>{release.published_by || "—"}</dd>
                    </dl>
                  </div>
                )}
              </div>
            )
          })}
        </TabsContent>

        <TabsContent value="diff">
          <Card size="sm">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-sm"><GitCompare className="h-4 w-4 text-muted-foreground" />{t("release.semanticDiff")}</CardTitle>
              <CardDescription className="text-xs">{t("release.semanticDiffDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              {readyReleases.length < 2 ? (
                <div className="rounded-lg border border-dashed px-4 py-7 text-center text-sm text-muted-foreground">
                  {t("release.requiresTwoVersions")}
                </div>
              ) : (
                <>
                  <div className="grid items-center gap-2 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]">
                    <Select value={fromId} onValueChange={(value) => { setFromId(value); setDiff(null) }}>
                      <SelectTrigger className="w-full"><SelectValue placeholder={t("release.fromVersion")} /></SelectTrigger>
                      <SelectContent>{readyReleases.map((item) => <SelectItem key={item.id} value={String(item.id)}>{releaseLabel(item)}</SelectItem>)}</SelectContent>
                    </Select>
                    <ArrowRight className="mx-auto hidden h-4 w-4 text-muted-foreground sm:block" />
                    <Select value={toId} onValueChange={(value) => { setToId(value); setDiff(null) }}>
                      <SelectTrigger className="w-full"><SelectValue placeholder={t("release.toVersion")} /></SelectTrigger>
                      <SelectContent>{readyReleases.map((item) => <SelectItem key={item.id} value={String(item.id)}>{releaseLabel(item)}</SelectItem>)}</SelectContent>
                    </Select>
                    <Button variant="outline" onClick={runDiff} disabled={!fromId || !toId || fromId === toId || busy !== null}>
                      {busy === "diff" ? <Loader2 className="animate-spin" /> : <GitCompare />}
                      {t("release.compare")}
                    </Button>
                  </div>
                  {diff && (
                    <div className="grid gap-3 md:grid-cols-3">
                      {(["tbox", "vocabulary", "abox"] as const).map((layer) => (
                        <div key={layer} className="rounded-lg border p-4">
                          <p className="text-xs font-medium uppercase text-muted-foreground">{t(`release.layer.${layer}`)}</p>
                          <p className="mt-2 text-lg font-semibold">
                            <span className="text-emerald-600">+{diff.layers[layer].added}</span>
                            <span className="mx-2 text-muted-foreground">/</span>
                            <span className="text-red-600">−{diff.layers[layer].removed}</span>
                          </p>
                        </div>
                      ))}
                    </div>
                  )}
                </>
              )}
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="exports" className="space-y-4">
          <Card size="sm">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-sm"><Download className="h-4 w-4 text-muted-foreground" />{t("release.exportCenter")}</CardTitle>
              <CardDescription className="text-xs">{t("release.exportsDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="grid gap-3 md:grid-cols-[minmax(15rem,1fr)_2fr] md:items-end">
                <div className="space-y-1.5">
                  <p className="text-xs font-medium">{t("release.exportSource")}</p>
                  <Select value={exportSource} onValueChange={setExportSource}>
                    <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem value="workspace">{t("release.currentWorkspace")}</SelectItem>
                      {readyReleases.map((item) => (
                        <SelectItem key={item.id} value={String(item.id)}>{releaseLabel(item)} · {t("release.immutableVersion")}</SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                  {(["tbox", "vocabulary", "abox", "bundle"] as ReleaseLayer[]).map((layer) => (
                    <Button key={layer} size="sm" variant={layer === "bundle" ? "default" : "outline"} disabled={busy !== null} onClick={() => startExport(layer)}>
                      {busy === `export-${layer}-${exportSource}` && <Loader2 className="animate-spin" />}
                      {t(`release.layer.${layer}`)}
                    </Button>
                  ))}
                </div>
              </div>
            </CardContent>
          </Card>

          <div className="space-y-2">
            <div>
              <h3 className="text-sm font-semibold">{t("release.exportHistory")}</h3>
              <p className="text-xs text-muted-foreground">{t("release.exportHistoryHelp")}</p>
            </div>
            {exports.length === 0 ? (
              <div className="rounded-lg border border-dashed px-4 py-8 text-center text-sm text-muted-foreground">
                {t("release.noExports")}
              </div>
            ) : exports.slice(0, 20).map((job) => {
              const expanded = expandedExportId === job.id
              const sourceRelease = job.release_id ? releases.find((item) => item.id === job.release_id) : null
              return (
                <div key={job.id} className="rounded-lg border bg-background">
                  <div className="flex flex-wrap items-center justify-between gap-3 p-3">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium">{t(`release.layer.${job.layer}`)}</span>
                        <Badge variant={job.status === "failed" ? "destructive" : job.status === "completed" ? "secondary" : "outline"}>
                          {(job.status === "running" || job.status === "pending") && <Loader2 className="animate-spin" />}
                          {t(`release.exportStatus.${job.status}`)}
                        </Badge>
                        <Badge variant="outline">{sourceRelease ? releaseLabel(sourceRelease) : t("release.currentWorkspaceShort")}</Badge>
                      </div>
                      <p className="mt-1 text-xs text-muted-foreground">
                        {job.processed_statements.toLocaleString()} {t("release.statementsTotal")} · {t("release.fileCount", { count: job.files.length })} · {new Date(job.created_at).toLocaleString(locale)}
                      </p>
                    </div>
                    {job.files.length > 0 && (
                      <Button size="sm" variant="ghost" onClick={() => setExpandedExportId(expanded ? null : job.id)}>
                        {expanded ? <ChevronUp /> : <ChevronDown />}
                        {expanded ? t("release.hideFiles") : t("release.showFiles")}
                      </Button>
                    )}
                  </div>
                  {job.error && <p className="border-t px-4 py-3 text-xs text-destructive">{job.error}</p>}
                  {expanded && (
                    <div className="grid gap-1 border-t bg-muted/15 p-3 sm:grid-cols-2">
                      {job.files.map((file) => (
                        <Button key={file.name} asChild size="sm" variant="ghost" className="h-auto min-w-0 justify-start py-2 text-xs">
                          <a href={api.exportFileUrl(ksId, job.id, file.name)} download>
                            <Download className="shrink-0" /><span className="truncate">{file.name}</span>
                          </a>
                        </Button>
                      ))}
                    </div>
                  )}
                </div>
              )
            })}
          </div>
        </TabsContent>
      </Tabs>
    </div>
  )
}
