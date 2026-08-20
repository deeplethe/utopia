import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useNavigate, useSearchParams } from "react-router-dom"
import { toast } from "sonner"
import {
  Check, ChevronLeft, ChevronRight, Eye, FileText, FileUp, Folder, FolderInput, FolderPlus, ListChecks,
  Loader2, Search, Sparkles, Trash2, Upload,
} from "lucide-react"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type { DocumentMeta } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Checkbox } from "@/components/ui/checkbox"
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import DeleteImpactDialog from "@/components/DeleteImpactDialog"
import ExtractDialog from "@/components/ExtractDialog"
import DocumentContributionDialog from "@/components/DocumentContributionDialog"

const PAGE_SIZE = 20

function humanSize(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

function formatTimestamp(value: string, locale: string): string {
  return new Date(value).toLocaleString(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  })
}

function StatusBadge({ status }: { status: DocumentMeta["parse_status"] }) {
  const { t } = useI18n()
  const map: Record<string, "secondary" | "default" | "destructive"> = {
    pending: "secondary", parsed: "default", failed: "destructive",
  }
  const label = {
    pending: t("documents.notParsed"), parsed: t("documents.parsed"), failed: t("documents.parseFailed"),
  }[status] ?? status
  return <Badge variant={map[status] ?? "secondary"}>{label}</Badge>
}

function ExtractionBadges({ doc }: { doc: DocumentMeta }) {
  const { locale, t } = useI18n()
  if (!doc.tbox_extracted_at && !doc.abox_extracted_at) {
    return <span className="text-xs text-muted-foreground">—</span>
  }
  return (
    <span className="inline-flex flex-wrap gap-1">
      {doc.tbox_extracted_at && (
        <Badge variant="secondary" className="gap-1 text-[10px]" title={t("documents.schemaExtracted", { time: new Date(doc.tbox_extracted_at).toLocaleString(locale) })}>
          <Check className="h-3 w-3" /> {t("documents.schema")}
        </Badge>
      )}
      {doc.abox_extracted_at && (
        <Badge variant="secondary" className="gap-1 text-[10px]" title={t("documents.instancesExtracted", { time: new Date(doc.abox_extracted_at).toLocaleString(locale) })}>
          <Check className="h-3 w-3" /> {t("documents.instances")}
        </Badge>
      )}
    </span>
  )
}

function childFolders(folders: string[], current: string): string[] {
  const prefix = current === "/" ? "/" : current + "/"
  const kids = new Set<string>()
  for (const f of folders) {
    if (f === current || !f.startsWith(prefix)) continue
    const seg = f.slice(prefix.length).split("/")[0]
    if (seg) kids.add(current === "/" ? "/" + seg : current + "/" + seg)
  }
  return [...kids].sort()
}

function breadcrumbs(folder: string, rootLabel: string): { label: string; path: string }[] {
  const crumbs = [{ label: rootLabel, path: "/" }]
  let acc = ""
  for (const p of folder.split("/").filter(Boolean)) {
    acc += "/" + p
    crumbs.push({ label: p, path: acc })
  }
  return crumbs
}

/** Themed folder picker: type a new path or choose an existing folder from a styled list.
 *  Replaces the native <datalist>, whose OS-rendered dropdown didn't match the app theme. */
function FolderCombobox({
  value, onChange, folders, placeholder,
}: {
  value: string
  onChange: (v: string) => void
  folders: string[]
  placeholder?: string
}) {
  const [open, setOpen] = useState(false)
  const term = value.trim().toLowerCase()
  const matches = folders.filter((f) => f.toLowerCase().includes(term) && f !== value).slice(0, 8)
  return (
    <div className="relative">
      <Input
        value={value}
        autoComplete="off"
        placeholder={placeholder}
        onChange={(e) => { onChange(e.target.value); setOpen(true) }}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 120)}
        onKeyDown={(e) => e.key === "Escape" && setOpen(false)}
      />
      {open && matches.length > 0 && (
        <ul className="absolute z-50 mt-1 max-h-56 w-full overflow-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-md">
          {matches.map((f) => (
            <li key={f}>
              <button
                type="button"
                className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent hover:text-accent-foreground"
                // onMouseDown (not onClick) so the selection registers before the input's blur closes the list.
                onMouseDown={(e) => { e.preventDefault(); onChange(f); setOpen(false) }}
              >
                <Folder className="h-3.5 w-3.5 shrink-0 text-primary" />
                <span className="truncate">{f}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

/** KS-scoped document manager. Read-only for viewers; write controls appear only when
 *  `canWrite` (editor/owner/admin). `onChanged` lets the parent refresh KS stats/sources. */
export default function KsDocuments({
  ksId, canWrite, onChanged,
}: {
  ksId: number
  canWrite: boolean
  onChanged?: () => void
}) {
  const { locale, t } = useI18n()
  const confirmAction = useConfirm()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const [docs, setDocs] = useState<DocumentMeta[]>([])
  const [docTotal, setDocTotal] = useState(0)
  const [serverFolders, setServerFolders] = useState<string[]>([])
  const [loading, setLoading] = useState(true)
  const [uploading, setUploading] = useState(false)
  const [parsing, setParsing] = useState<number | null>(null)
  const [batchParsing, setBatchParsing] = useState(false)
  const [dragOver, setDragOver] = useState(false)
  const [cwd, setCwd] = useState(() => searchParams.get("folder") || "/")
  const [virtualFolders, setVirtualFolders] = useState<string[]>([])
  const [search, setSearch] = useState("")
  const [debouncedSearch, setDebouncedSearch] = useState("")
  const [page, setPage] = useState(0)
  const [selectedDocIds, setSelectedDocIds] = useState<Set<number>>(new Set())
  const [selectedFolders, setSelectedFolders] = useState<Set<string>>(new Set())
  const [moveDoc, setMoveDoc] = useState<DocumentMeta | null>(null)
  const [moveTarget, setMoveTarget] = useState("/")
  const [deleteDoc, setDeleteDoc] = useState<DocumentMeta | null>(null)
  const [extractDoc, setExtractDoc] = useState<DocumentMeta | null>(null)
  const [contributionDoc, setContributionDoc] = useState<DocumentMeta | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const versionRef = useRef<HTMLInputElement>(null)
  const versionDoc = useRef<DocumentMeta | null>(null)
  const dragDepthRef = useRef(0)

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      const result = await api.listDocumentsPage(ksId, {
        folder: debouncedSearch ? undefined : cwd,
        q: debouncedSearch || undefined,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      })
      setDocs(result.items)
      setDocTotal(result.total)
      setServerFolders(result.folders)
    } catch (e) {
      toast.error(t("documents.loadFailed", { error: (e as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [cwd, debouncedSearch, ksId, page, t])

  useEffect(() => { refresh() }, [refresh])
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedSearch(search.trim()), 300)
    return () => clearTimeout(timer)
  }, [search])
  useEffect(() => { setPage(0) }, [cwd, debouncedSearch])
  useEffect(() => {
    setPage((current) => Math.min(current, Math.max(0, Math.ceil(docTotal / PAGE_SIZE) - 1)))
  }, [docTotal])

  const allFolders = useMemo(
    () => [...new Set([...serverFolders, ...virtualFolders, "/"])],
    [serverFolders, virtualFolders],
  )
  // When searching, flatten across ALL folders and hide subfolders; otherwise browse the tree.
  const searching = debouncedSearch.length > 0
  const subfolders = useMemo(() => (searching ? [] : childFolders(allFolders, cwd)), [searching, allFolders, cwd])
  const docsHere = docs
  const pageCount = Math.max(1, Math.ceil(docTotal / PAGE_SIZE))
  const selectableCount = docsHere.length + subfolders.length
  const allCurrentSelected = selectableCount > 0
    && docsHere.every((doc) => selectedDocIds.has(doc.id))
    && subfolders.every((folder) => selectedFolders.has(folder))
  const selectedCount = selectedDocIds.size + selectedFolders.size

  const changeFolder = (folder: string) => {
    setCwd(folder)
    const next = new URLSearchParams(searchParams)
    if (folder === "/") next.delete("folder")
    else next.set("folder", folder)
    setSearchParams(next, { replace: true })
  }

  const afterChange = useCallback(() => { refresh(); onChanged?.() }, [refresh, onChanged])

  const toggleDocument = (documentId: number) => {
    setSelectedDocIds((current) => {
      const next = new Set(current)
      if (next.has(documentId)) next.delete(documentId)
      else next.add(documentId)
      return next
    })
  }

  const toggleFolder = (folder: string) => {
    setSelectedFolders((current) => {
      const next = new Set(current)
      if (next.has(folder)) next.delete(folder)
      else next.add(folder)
      return next
    })
  }

  const toggleCurrentPage = () => {
    setSelectedDocIds((current) => {
      const next = new Set(current)
      for (const doc of docsHere) {
        if (allCurrentSelected) next.delete(doc.id)
        else next.add(doc.id)
      }
      return next
    })
    setSelectedFolders((current) => {
      const next = new Set(current)
      for (const folder of subfolders) {
        if (allCurrentSelected) next.delete(folder)
        else next.add(folder)
      }
      return next
    })
  }

  const runBatchParse = useCallback(async (
    documentIds: number[],
    folders: string[],
    clearSelection = false,
  ) => {
    if (documentIds.length === 0 && folders.length === 0) return
    setBatchParsing(true)
    try {
      const result = await api.parseDocuments(ksId, {
        document_ids: documentIds,
        folders,
        recursive: true,
      })
      if (result.total === 0) toast.info(t("documents.nothingToParse"))
      else toast.success(t("documents.batchParsed", {
        parsed: result.parsed,
        failed: result.failed,
        total: result.total,
      }))
      if (clearSelection) {
        setSelectedDocIds(new Set())
        setSelectedFolders(new Set())
      }
      afterChange()
    } catch (error) {
      toast.error(t("documents.batchParseFailed", { error: (error as Error).message }))
    } finally {
      setBatchParsing(false)
    }
  }, [afterChange, ksId, t])

  const parseSelected = async () => {
    if (selectedFolders.size > 0 && !await confirmAction(t("documents.parseSelectionConfirm", {
      files: selectedDocIds.size,
      folders: selectedFolders.size,
    }))) return
    void runBatchParse([...selectedDocIds], [...selectedFolders], true)
  }

  const parseCurrentFolder = async () => {
    if (!await confirmAction(t("documents.parseFolderConfirm", { folder: cwd }))) return
    void runBatchParse([], [cwd])
  }

  const uploadFiles = useCallback(
    async (files: FileList | File[], folder: string) => {
      setUploading(true)
      let ok = 0
      for (const file of Array.from(files)) {
        try { await api.uploadDocument(ksId, file, folder); ok++ }
        catch (e) { toast.error(t("documents.uploadFailed", { name: file.name, error: (e as Error).message })) }
      }
      setUploading(false)
      if (ok) toast.success(t("documents.uploaded", { count: ok, folder }))
      afterChange()
    },
    [ksId, afterChange, t],
  )

  useEffect(() => {
    if (!canWrite) return

    const hasFiles = (event: DragEvent) => Array.from(event.dataTransfer?.types ?? []).includes("Files")
    const onDragEnter = (event: DragEvent) => {
      if (!hasFiles(event)) return
      event.preventDefault()
      dragDepthRef.current += 1
      setDragOver(true)
    }
    const onDragOver = (event: DragEvent) => {
      if (!hasFiles(event)) return
      event.preventDefault()
      if (event.dataTransfer) event.dataTransfer.dropEffect = "copy"
    }
    const onDragLeave = (event: DragEvent) => {
      if (dragDepthRef.current === 0) return
      event.preventDefault()
      dragDepthRef.current = Math.max(0, dragDepthRef.current - 1)
      if (dragDepthRef.current === 0) setDragOver(false)
    }
    const onDrop = (event: DragEvent) => {
      if (!hasFiles(event)) return
      event.preventDefault()
      dragDepthRef.current = 0
      setDragOver(false)
      if (!uploading && event.dataTransfer?.files.length) {
        void uploadFiles(event.dataTransfer.files, cwd)
      }
    }

    window.addEventListener("dragenter", onDragEnter)
    window.addEventListener("dragover", onDragOver)
    window.addEventListener("dragleave", onDragLeave)
    window.addEventListener("drop", onDrop)
    return () => {
      window.removeEventListener("dragenter", onDragEnter)
      window.removeEventListener("dragover", onDragOver)
      window.removeEventListener("dragleave", onDragLeave)
      window.removeEventListener("drop", onDrop)
    }
  }, [canWrite, cwd, uploadFiles, uploading])

  const parse = useCallback(
    async (doc: DocumentMeta) => {
      setParsing(doc.id)
      try {
        const res = await api.parseDocument(ksId, doc.id)
        if (res.parse_status === "failed") toast.error(t("documents.parseFailedDetail", { error: res.error ?? "" }))
        else toast.success(t("documents.parsedDetail", { count: res.chunk_count, parser: res.parser_backend ?? "" }))
        afterChange()
      } catch (e) {
        toast.error(t("documents.parseError", { error: (e as Error).message }))
      } finally {
        setParsing(null)
      }
    },
    [ksId, afterChange, t],
  )

  const newFolder = () => {
    const name = prompt(t("documents.newFolderPrompt"))?.trim()
    if (!name) return
    const path = cwd === "/" ? "/" + name : cwd + "/" + name
    setVirtualFolders((v) => [...v, path])
    changeFolder(path)
  }

  const doMove = useCallback(async () => {
    if (!moveDoc) return
    try {
      await api.moveDocument(ksId, moveDoc.id, moveTarget)
      toast.success(t("documents.moved"))
      setMoveDoc(null)
      afterChange()
    } catch (e) {
      toast.error(t("documents.moveFailed", { error: (e as Error).message }))
    }
  }, [ksId, moveDoc, moveTarget, afterChange, t])

  const startNewVersion = (doc: DocumentMeta) => {
    versionDoc.current = doc
    versionRef.current?.click()
  }
  const onNewVersionPicked = useCallback(async (files: FileList | null) => {
    const old = versionDoc.current
    if (!files || !files[0] || !old) return
    await uploadFiles([files[0]], old.folder)
    // Then let the user review the old version's ontology contribution and remove it.
    setDeleteDoc(old)
  }, [uploadFiles])

  const colSpan = 9

  return (
    <div className="min-w-0 space-y-4">
      <div className="flex min-w-0 items-center gap-2 overflow-x-auto pb-1">
        <div className="mr-auto flex shrink-0 items-center gap-1 text-sm">
          {breadcrumbs(cwd, t("documents.root")).map((c, i) => (
            <span key={c.path} className="flex items-center gap-1">
              {i > 0 && <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />}
              <button
                onClick={() => changeFolder(c.path)}
                className={i === breadcrumbs(cwd, t("documents.root")).length - 1 ? "font-medium" : "text-muted-foreground hover:text-foreground"}
              >
                {c.label}
              </button>
            </span>
          ))}
        </div>
        <div className="relative shrink-0">
          <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input value={search} onChange={(e) => setSearch(e.target.value)} placeholder={t("documents.search")} className="h-8 w-48 pl-7 text-sm" />
        </div>
        {canWrite && (
          <>
            <Button className="shrink-0" variant="outline" size="sm" onClick={newFolder}>
              <FolderPlus className="h-4 w-4" /> {t("documents.newFolder")}
            </Button>
            <Button className="shrink-0" size="sm" variant="outline" onClick={() => inputRef.current?.click()} disabled={uploading}>
              {uploading ? <Loader2 className="h-4 w-4 animate-spin" /> : <Upload className="h-4 w-4" />} {t("documents.uploadHere")}
            </Button>
            <Button className="shrink-0" size="sm" variant="outline" onClick={toggleCurrentPage}
              disabled={batchParsing || loading || selectableCount === 0}>
              <ListChecks className="h-4 w-4" />
              {t(allCurrentSelected ? "documents.clearCurrentPage" : "documents.selectCurrentPage")}
            </Button>
            {selectedCount > 0 && (
              <Badge className="shrink-0" variant="secondary">{t("documents.selectedCount", { count: selectedCount })}</Badge>
            )}
            <Button className="shrink-0" size="sm" variant="outline" onClick={parseCurrentFolder}
              disabled={batchParsing || searching}>
              {batchParsing ? <Loader2 className="h-4 w-4 animate-spin" /> : <FileText className="h-4 w-4" />}
              {t("documents.parseCurrentFolder")}
            </Button>
            <Button className="shrink-0" size="sm" onClick={parseSelected} disabled={batchParsing || selectedCount === 0}>
              {batchParsing ? <Loader2 className="h-4 w-4 animate-spin" /> : <ListChecks className="h-4 w-4" />}
              {t("documents.parseSelected", { count: selectedCount })}
            </Button>
          </>
        )}
      </div>

      <input
        ref={inputRef} type="file" multiple className="hidden"
        accept=".pdf,.docx,.doc,.xlsx,.xls,.txt,.md,.csv"
        onChange={(e) => { if (e.target.files) void uploadFiles(e.target.files, cwd); e.target.value = "" }}
      />
      <input
        ref={versionRef} type="file" className="hidden"
        accept=".pdf,.docx,.doc,.xlsx,.xls,.txt,.md,.csv"
        onChange={(e) => { onNewVersionPicked(e.target.files); e.target.value = "" }}
      />

      {dragOver && (
        <div className="pointer-events-none fixed inset-0 z-[100] flex items-center justify-center bg-background/80 backdrop-blur-sm">
          <div className="flex items-center gap-3 rounded-xl border bg-card px-6 py-4 text-sm font-medium shadow-lg">
            <Upload className="h-5 w-5 text-primary" />
            {t("documents.dropUpload", { folder: cwd })}
          </div>
        </div>
      )}

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-10" />
              <TableHead>{t("common.name")}</TableHead>
              <TableHead>{t("common.type")}</TableHead>
              <TableHead>{t("documents.size")}</TableHead>
              <TableHead>{t("documents.uploadedAt")}</TableHead>
              <TableHead>{t("common.status")}</TableHead>
              <TableHead>{t("documents.chunks")}</TableHead>
              <TableHead>{t("documents.extraction")}</TableHead>
              <TableHead className="text-right">{t("common.actions")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {subfolders.map((f) => (
              <TableRow key={f}
                className={"cursor-pointer " + (selectedFolders.has(f) ? "bg-muted/50" : "")}
                onClick={() => changeFolder(f)}>
                <TableCell onClick={(event) => event.stopPropagation()}>
                  {canWrite && (
                    <Checkbox checked={selectedFolders.has(f)} onCheckedChange={() => toggleFolder(f)}
                      disabled={batchParsing}
                      aria-label={t("documents.selectFolder", { name: f.split("/").pop() ?? f })} />
                  )}
                </TableCell>
                <TableCell className="font-medium">
                  <span className="inline-flex items-center gap-2">
                    <Folder className="h-4 w-4 text-primary" />
                    {f.split("/").pop()}
                  </span>
                </TableCell>
                <TableCell className="text-muted-foreground">{t("documents.folder")}</TableCell>
                <TableCell>—</TableCell><TableCell>—</TableCell><TableCell>—</TableCell>
                <TableCell>—</TableCell><TableCell>—</TableCell>
                <TableCell className="text-right text-xs text-muted-foreground">{t("documents.open")}</TableCell>
              </TableRow>
            ))}

            {loading ? (
              <TableRow><TableCell colSpan={colSpan} className="h-20 text-center text-muted-foreground">{t("common.loading")}</TableCell></TableRow>
            ) : subfolders.length === 0 && docsHere.length === 0 ? (
              <TableRow><TableCell colSpan={colSpan} className="h-20 text-center text-muted-foreground">
                {searching
                  ? t("documents.noMatches")
                  : canWrite ? t("documents.emptyWritable") : t("documents.empty")}
              </TableCell></TableRow>
            ) : (
              docsHere.map((doc) => (
                <TableRow key={doc.id}
                  className={"cursor-pointer " + (selectedDocIds.has(doc.id) ? "bg-muted/50" : "")}
                  onClick={() => navigate(`/knowledge/${ksId}/documents/${doc.id}`, { state: { folder: cwd } })}>
                  <TableCell onClick={(event) => event.stopPropagation()}>
                    {canWrite && (
                      <Checkbox checked={selectedDocIds.has(doc.id)} onCheckedChange={() => toggleDocument(doc.id)}
                        disabled={batchParsing}
                        aria-label={t("documents.selectDocument", { name: doc.original_filename })} />
                    )}
                  </TableCell>
                  <TableCell className="max-w-xs truncate font-medium">
                    <span className="inline-flex items-center gap-2">
                      <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
                      {doc.original_filename}
                    </span>
                  </TableCell>
                  <TableCell><Badge variant="outline" className="uppercase">{doc.ext}</Badge></TableCell>
                  <TableCell className="text-muted-foreground">{humanSize(doc.size_bytes)}</TableCell>
                  <TableCell className="whitespace-nowrap text-xs text-muted-foreground">
                    {formatTimestamp(doc.uploaded_at, locale)}
                  </TableCell>
                  <TableCell><StatusBadge status={doc.parse_status} /></TableCell>
                  <TableCell className="text-muted-foreground">{doc.chunk_count || "—"}</TableCell>
                  <TableCell><ExtractionBadges doc={doc} /></TableCell>
                  <TableCell className="space-x-1 text-right" onClick={(e) => e.stopPropagation()}>
                    {canWrite && (
                      <Button size="sm" variant="outline" disabled={batchParsing || parsing === doc.id} onClick={() => parse(doc)}>
                        {parsing === doc.id && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
                        {doc.parse_status === "parsed" ? t("documents.reparse") : t("documents.parse")}
                      </Button>
                    )}
                    {canWrite && doc.parse_status === "parsed" && doc.chunk_count > 0 && (
                      <Button size="sm" title={t("documents.extractTitle")} onClick={() => setExtractDoc(doc)}>
                        <Sparkles className="h-3.5 w-3.5" /> {t("documents.extract")}
                      </Button>
                    )}
                    {(doc.tbox_extracted_at || doc.abox_extracted_at) && (
                      <Button size="icon" variant="ghost" className="h-8 w-8" title={t("documents.viewContribution")} onClick={() => setContributionDoc(doc)}>
                        <Eye className="h-4 w-4" />
                      </Button>
                    )}
                    {canWrite && (
                      <>
                        <Button size="icon" variant="ghost" className="h-8 w-8" title={t("documents.uploadVersion")} onClick={() => startNewVersion(doc)}>
                          <FileUp className="h-4 w-4" />
                        </Button>
                        <Button size="icon" variant="ghost" className="h-8 w-8" title={t("documents.move")} onClick={() => { setMoveDoc(doc); setMoveTarget(doc.folder) }}>
                          <FolderInput className="h-4 w-4" />
                        </Button>
                        <Button size="icon" variant="ghost" className="h-8 w-8 text-muted-foreground hover:text-destructive" title={t("common.delete")} onClick={() => setDeleteDoc(doc)}>
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </>
                    )}
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>

      {docTotal > PAGE_SIZE && (
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>{t("review.page", {
            start: page * PAGE_SIZE + 1,
            end: Math.min(docTotal, (page + 1) * PAGE_SIZE),
            total: docTotal,
          })}</span>
          <div className="flex gap-1">
            <Button size="icon" variant="outline" className="h-7 w-7"
              disabled={loading || page === 0} onClick={() => setPage((current) => current - 1)}>
              <ChevronLeft className="h-4 w-4" />
            </Button>
            <Button size="icon" variant="outline" className="h-7 w-7"
              disabled={loading || page >= pageCount - 1}
              onClick={() => setPage((current) => current + 1)}>
              <ChevronRight className="h-4 w-4" />
            </Button>
          </div>
        </div>
      )}

      {/* Per-document extraction (schema / instances / both), the doc's chunks pre-selected */}
      {extractDoc && (
        <ExtractDialog
          ksId={ksId}
          open={!!extractDoc}
          onOpenChange={(o) => !o && setExtractDoc(null)}
          mode="both"
          selectableModes={["both", "tbox", "abox"]}
          presetDocId={extractDoc.id}
          onStarted={() => { setExtractDoc(null); afterChange() }}
        />
      )}

      <DocumentContributionDialog
        ksId={ksId}
        document={contributionDoc}
        open={!!contributionDoc}
        onOpenChange={(open) => { if (!open) setContributionDoc(null) }}
      />

      {/* Move dialog */}
      <Dialog open={!!moveDoc} onOpenChange={(o) => !o && setMoveDoc(null)}>
        <DialogContent>
          <DialogHeader><DialogTitle>{t("documents.moveTitle", { name: moveDoc?.original_filename ?? "" })}</DialogTitle></DialogHeader>
          <div className="space-y-2 py-2">
            <Label>{t("documents.targetFolder")}</Label>
            <FolderCombobox value={moveTarget} onChange={setMoveTarget} folders={allFolders} placeholder="/manuals/pumps" />
            <p className="text-xs text-muted-foreground">{t("documents.virtualFolderHelp")}</p>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setMoveDoc(null)}>{t("common.cancel")}</Button>
            <Button onClick={doMove}>{t("documents.move")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Delete-with-impact dialog */}
      <DeleteImpactDialog
        ksId={ksId} doc={deleteDoc} open={!!deleteDoc}
        onOpenChange={(o) => !o && setDeleteDoc(null)} onDeleted={afterChange}
      />
    </div>
  )
}
