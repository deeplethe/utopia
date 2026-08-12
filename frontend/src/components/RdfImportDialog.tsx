import { useCallback, useEffect, useState } from "react"
import { CheckCircle2, FileUp, Loader2, TriangleAlert } from "lucide-react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import type {
  RdfImportFormat,
  RdfImportResult,
  RdfImportStrategy,
  RdfImportTarget,
} from "@/lib/types"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"

function ResultMetric({ label, value, detail }: { label: string; value: number; detail?: string }) {
  const { locale } = useI18n()
  return (
    <div className="rounded-lg border bg-muted/20 px-3 py-2.5">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="mt-0.5 text-xl font-semibold tabular-nums">{value.toLocaleString(locale)}</div>
      {detail && <div className="mt-0.5 text-[11px] text-muted-foreground">{detail}</div>}
    </div>
  )
}

export default function RdfImportDialog({
  ksId,
  baseIri,
  open,
  onOpenChange,
  onImported,
}: {
  ksId: number
  baseIri: string
  open: boolean
  onOpenChange: (open: boolean) => void
  onImported: (result: RdfImportResult) => void
}) {
  const { locale, t } = useI18n()
  const formatLabels: Record<RdfImportFormat, string> = {
    auto: t("rdf.format.auto"), turtle: "Turtle", rdfxml: "RDF/XML", ntriples: "N-Triples", jsonld: "JSON-LD",
  }
  const targetHelp: Record<RdfImportTarget, string> = {
    auto: t("rdf.targetHelp.auto"), tbox: t("rdf.targetHelp.tbox"), abox: t("rdf.targetHelp.abox"),
  }
  const [file, setFile] = useState<File | null>(null)
  const [fileInputKey, setFileInputKey] = useState(0)
  const [target, setTarget] = useState<RdfImportTarget>("auto")
  const [strategy, setStrategy] = useState<RdfImportStrategy>("merge")
  const [rdfFormat, setRdfFormat] = useState<RdfImportFormat>("auto")
  const [customBaseIri, setCustomBaseIri] = useState("")
  const [running, setRunning] = useState(false)
  const [result, setResult] = useState<RdfImportResult | null>(null)

  const reset = useCallback(() => {
    setFile(null)
    setFileInputKey((value) => value + 1)
    setTarget("auto")
    setStrategy("merge")
    setRdfFormat("auto")
    setCustomBaseIri("")
    setRunning(false)
    setResult(null)
  }, [])

  useEffect(() => {
    if (open) reset()
  }, [open, reset])

  const run = useCallback(async () => {
    if (!file) return
    setRunning(true)
    try {
      const imported = await api.importRdf(ksId, file, {
        target,
        strategy,
        format: rdfFormat,
        baseIri: customBaseIri || undefined,
      })
      setResult(imported)
      onImported(imported)
      const changed = imported.tbox_added + imported.tbox_removed + imported.abox_added + imported.abox_removed
      toast.success(changed
        ? t("rdf.imported", { count: imported.parsed_triples.toLocaleString(locale) })
        : t("rdf.alreadyMatched"))
    } catch (error) {
      toast.error(t("rdf.failed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setRunning(false)
    }
  }, [file, ksId, target, strategy, rdfFormat, customBaseIri, onImported, locale, t])

  const replaceScope = target === "auto"
    ? t("rdf.scope.all")
    : target === "tbox"
      ? t("rdf.scope.tbox")
      : t("rdf.scope.abox")

  return (
    <Dialog open={open} onOpenChange={(next) => !running && onOpenChange(next)}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("rdf.title")}</DialogTitle>
          <DialogDescription>{t("rdf.description")}</DialogDescription>
        </DialogHeader>

        {result ? (
          <div className="space-y-4">
            <Alert className="border-emerald-500/30 bg-emerald-500/5">
              <CheckCircle2 className="text-emerald-600" />
              <AlertTitle>{t("rdf.complete")}</AlertTitle>
              <AlertDescription>
                {t("rdf.completeDescription", { name: result.filename, format: formatLabels[result.format] })}
              </AlertDescription>
            </Alert>
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
              <ResultMetric label={t("rdf.parsedTriples")} value={result.parsed_triples} />
              <ResultMetric label={t("rdf.ontology")} value={result.tbox_triples} detail={`+${result.tbox_added} / −${result.tbox_removed}`} />
              <ResultMetric label={t("rdf.instances")} value={result.abox_triples} detail={`+${result.abox_added} / −${result.abox_removed}`} />
            </div>
            <div className="flex flex-wrap gap-2 text-xs">
              <Badge variant={result.open_conflicts.length ? "destructive" : "secondary"}>
                {t("rdf.openConflicts", { count: result.open_conflicts.length })}
              </Badge>
              <Badge variant={result.validation.counts.error ? "destructive" : "secondary"}>
                {t("rdf.validationErrors", { count: result.validation.counts.error })}
              </Badge>
              <Badge variant="outline">{t("rdf.validationWarnings", { count: result.validation.counts.warning })}</Badge>
              <Badge variant="outline">{result.strategy === "merge" ? t("rdf.merged") : t("rdf.replaced")}</Badge>
            </div>
          </div>
        ) : (
          <div className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="rdf-file">{t("rdf.file")}</Label>
              <Input
                key={fileInputKey}
                id="rdf-file"
                type="file"
                accept=".ttl,.rdf,.owl,.xml,.nt,.jsonld,.json"
                onChange={(event) => setFile(event.target.files?.[0] ?? null)}
              />
              <p className="text-xs text-muted-foreground">{t("rdf.fileHelp")}</p>
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1.5">
                <Label>{t("rdf.destination")}</Label>
                <Select value={target} onValueChange={(value) => setTarget(value as RdfImportTarget)}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="auto">{t("rdf.target.auto")}</SelectItem>
                    <SelectItem value="tbox">{t("rdf.target.tbox")}</SelectItem>
                    <SelectItem value="abox">{t("rdf.target.abox")}</SelectItem>
                  </SelectContent>
                </Select>
                <p className="text-xs text-muted-foreground">{targetHelp[target]}</p>
              </div>
              <div className="space-y-1.5">
                <Label>{t("rdf.writeMode")}</Label>
                <Select value={strategy} onValueChange={(value) => setStrategy(value as RdfImportStrategy)}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="merge">{t("rdf.merge")}</SelectItem>
                    <SelectItem value="replace">{t("rdf.replace")}</SelectItem>
                  </SelectContent>
                </Select>
                <p className="text-xs text-muted-foreground">{t("rdf.writeHelp")}</p>
              </div>
              <div className="space-y-1.5">
                <Label>{t("rdf.syntax")}</Label>
                <Select value={rdfFormat} onValueChange={(value) => setRdfFormat(value as RdfImportFormat)}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    {Object.entries(formatLabels).map(([value, label]) => (
                      <SelectItem key={value} value={value}>{label}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="rdf-base">{t("rdf.baseIri")}</Label>
                <Input
                  id="rdf-base"
                  value={customBaseIri}
                  onChange={(event) => setCustomBaseIri(event.target.value)}
                  placeholder={baseIri}
                />
                <p className="text-xs text-muted-foreground">{t("rdf.baseIriHelp")}</p>
              </div>
            </div>

            {strategy === "replace" && (
              <Alert variant="destructive">
                <TriangleAlert />
                <AlertTitle>{t("rdf.replaceTitle", { scope: replaceScope })}</AlertTitle>
                <AlertDescription>{t("rdf.replaceDescription")}</AlertDescription>
              </Alert>
            )}
          </div>
        )}

        <DialogFooter>
          {result ? (
            <>
              <Button variant="outline" onClick={reset}>{t("rdf.importAnother")}</Button>
              <Button onClick={() => onOpenChange(false)}>{t("common.done")}</Button>
            </>
          ) : (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)} disabled={running}>{t("common.cancel")}</Button>
              <Button variant={strategy === "replace" ? "destructive" : "default"} onClick={run} disabled={!file || running}>
                {running ? <Loader2 className="h-4 w-4 animate-spin" /> : <FileUp className="h-4 w-4" />}
                {running ? t("rdf.importing") : strategy === "replace" ? t("rdf.replaceImport") : t("rdf.import")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
