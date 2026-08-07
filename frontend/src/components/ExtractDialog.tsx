import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { Loader2, Sparkles } from "lucide-react"
import { api } from "@/lib/api"
import type { Chunk, DocumentMeta, ExtractionJob } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Label } from "@/components/ui/label"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"

const SYS_MODEL = "__default__" // use the knowledge system's configured model

type Mode = "tbox" | "abox" | "both"

const MODE_LABEL: Record<Mode, string> = {
  tbox: "Schema only (TBox)",
  abox: "Instances only (ABox)",
  both: "Schema + Instances",
}

export default function ExtractDialog({
  ksId,
  open,
  onOpenChange,
  onStarted,
  mode = "tbox",
  selectableModes,
  presetDocId,
}: {
  ksId: number
  open: boolean
  onOpenChange: (o: boolean) => void
  onStarted: (job: ExtractionJob) => void
  mode?: Mode
  /** If given (2+ modes), a selector lets the user choose; otherwise `mode` is fixed. */
  selectableModes?: Mode[]
  /** Opened from a document row: pre-select that doc and check all its chunks. */
  presetDocId?: number
}) {
  const [activeMode, setActiveMode] = useState<Mode>(mode)
  const [docs, setDocs] = useState<DocumentMeta[]>([])
  const [docId, setDocId] = useState<number | null>(null)
  const [chunks, setChunks] = useState<Chunk[]>([])
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [models, setModels] = useState<string[]>([])
  const [model, setModel] = useState(SYS_MODEL)
  const [running, setRunning] = useState(false)

  // Reset — or preset to a given document (per-row "Extract") — each time the dialog opens.
  useEffect(() => {
    if (!open) return
    setActiveMode(mode)
    setModel(SYS_MODEL)
    setRunning(false)
    if (presetDocId) {
      setDocId(presetDocId)
      api.getChunks(ksId, presetDocId)
        .then((cs) => { setChunks(cs); setSelected(new Set(cs.map((c) => c.id))) })
        .catch((e) => toast.error(`Failed to load chunks: ${(e as Error).message}`))
    } else {
      setDocId(null)
      setChunks([])
      setSelected(new Set())
    }
  }, [open, mode, presetDocId, ksId])

  useEffect(() => {
    if (!open) return
    api.listDocuments(ksId)
      .then((all) => setDocs(all.filter((d) => d.parse_status === "parsed" && d.chunk_count > 0)))
      .catch((e) => toast.error(`Failed to load documents: ${(e as Error).message}`))
    api.getModels().then((m) => setModels(m.models)).catch(() => {})
  }, [open, ksId])

  const selectDoc = useCallback(async (id: number) => {
    setDocId(id)
    setSelected(new Set())
    try {
      setChunks(await api.getChunks(ksId, id))
    } catch (e) {
      toast.error(`Failed to load chunks: ${(e as Error).message}`)
    }
  }, [ksId])

  const toggle = (id: number) =>
    setSelected((prev) => {
      const next = new Set(prev)
      next.has(id) ? next.delete(id) : next.add(id)
      return next
    })

  const allSelected = chunks.length > 0 && selected.size === chunks.length
  const toggleAll = () =>
    setSelected(allSelected ? new Set() : new Set(chunks.map((c) => c.id)))

  const run = useCallback(async () => {
    if (selected.size === 0) return
    setRunning(true)
    try {
      const ids = chunks.filter((c) => selected.has(c.id)).map((c) => c.id)
      const m = model === SYS_MODEL ? undefined : model
      const job = activeMode === "abox"
        ? await api.extractInstances(ksId, ids, m)
        : activeMode === "both"
          ? await api.extractAll(ksId, ids, m)
          : await api.runExtraction(ksId, ids, m)
      toast.info(`Extraction started, processing ${ids.length} chunks in the background…`)
      onStarted(job)
      onOpenChange(false)
    } catch (e) {
      toast.error(`Failed to start extraction: ${(e as Error).message}`)
    } finally {
      setRunning(false)
    }
  }, [selected, chunks, ksId, model, activeMode, onStarted, onOpenChange])

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Extract from Documents</DialogTitle>
          <DialogDescription>
            {activeMode === "abox"
              ? "An LLM extracts specific individuals typed by this ontology's classes, resolving each against existing instances (ambiguous ones go to the resolution queue)."
              : activeMode === "both"
                ? "An LLM first extracts the schema (TBox), then the specific individuals (ABox) that fit it — in one run."
                : "An LLM extracts the ontology schema (classes, properties, axioms) and merges it into this knowledge system."}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {selectableModes && selectableModes.length > 1 && (
            <div className="space-y-1.5">
              <Label className="text-xs">What to extract</Label>
              <Select value={activeMode} onValueChange={(v) => setActiveMode(v as Mode)}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {selectableModes.map((m) => <SelectItem key={m} value={m}>{MODE_LABEL[m]}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
          )}
          <div className="grid grid-cols-2 gap-3">
            <div className="min-w-0 space-y-1.5">
              <Label className="text-xs">Document</Label>
              <Select value={docId ? String(docId) : ""} onValueChange={(v) => selectDoc(Number(v))}>
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="Select a parsed document" />
                </SelectTrigger>
                <SelectContent>
                  {docs.map((d) => (
                    <SelectItem key={d.id} value={String(d.id)}>
                      {d.original_filename} ({d.chunk_count} chunks)
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="min-w-0 space-y-1.5">
              <Label className="text-xs">Model</Label>
              <Select value={model} onValueChange={setModel}>
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={SYS_MODEL}>System default</SelectItem>
                  {models.map((m) => (
                    <SelectItem key={m} value={m}>{m}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>

          {docs.length === 0 && (
            <p className="text-sm text-muted-foreground">
              This knowledge system has no parsed documents yet. Upload and parse them in the Documents tab first.
            </p>
          )}

          {chunks.length > 0 && (
            <div className="rounded-md border">
              <div className="flex items-center gap-2 border-b px-3 py-2">
                <Checkbox checked={allSelected} onCheckedChange={toggleAll} id="all" />
                <Label htmlFor="all" className="text-xs font-medium">
                  Select all ({selected.size}/{chunks.length} selected)
                </Label>
              </div>
              <ScrollArea className="h-64">
                <div className="divide-y">
                  {chunks.map((c) => (
                    <label key={c.id} className="flex cursor-pointer items-start gap-2 px-3 py-2 hover:bg-muted/50">
                      <Checkbox checked={selected.has(c.id)} onCheckedChange={() => toggle(c.id)} className="mt-0.5" />
                      <div className="min-w-0">
                        <div className="text-[10px] text-muted-foreground">#{c.idx} · ~{c.token_estimate} tokens</div>
                        <div className="line-clamp-2 text-xs">{c.text}</div>
                      </div>
                    </label>
                  ))}
                </div>
              </ScrollArea>
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={run} disabled={running || selected.size === 0}>
            {running ? <Loader2 className="h-4 w-4 animate-spin" /> : <Sparkles className="h-4 w-4" />}
            Extract {selected.size > 0 ? `(${selected.size})` : ""}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
