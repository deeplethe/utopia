import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { toast } from "sonner"
import {
  Check, ChevronRight, FileText, FileUp, Folder, FolderInput, FolderPlus, Loader2, Search, Sparkles, Trash2, Upload,
} from "lucide-react"
import { api } from "@/lib/api"
import type { DocumentMeta } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import DeleteImpactDialog from "@/components/DeleteImpactDialog"
import DocumentDetailSheet from "@/components/DocumentDetailSheet"
import ExtractDialog from "@/components/ExtractDialog"

function humanSize(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

function StatusBadge({ status }: { status: DocumentMeta["parse_status"] }) {
  const map: Record<string, "secondary" | "default" | "destructive"> = {
    pending: "secondary", parsed: "default", failed: "destructive",
  }
  const label = { pending: "Not parsed", parsed: "Parsed", failed: "Parse failed" }[status] ?? status
  return <Badge variant={map[status] ?? "secondary"}>{label}</Badge>
}

function ExtractionBadges({ doc }: { doc: DocumentMeta }) {
  if (!doc.tbox_extracted_at && !doc.abox_extracted_at) {
    return <span className="text-xs text-muted-foreground">—</span>
  }
  return (
    <span className="inline-flex flex-wrap gap-1">
      {doc.tbox_extracted_at && (
        <Badge variant="secondary" className="gap-1 text-[10px]" title={`Schema extracted ${new Date(doc.tbox_extracted_at).toLocaleString()}`}>
          <Check className="h-3 w-3" /> Schema
        </Badge>
      )}
      {doc.abox_extracted_at && (
        <Badge variant="secondary" className="gap-1 text-[10px]" title={`Instances extracted ${new Date(doc.abox_extracted_at).toLocaleString()}`}>
          <Check className="h-3 w-3" /> Instances
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

function breadcrumbs(folder: string): { label: string; path: string }[] {
  const crumbs = [{ label: "Root", path: "/" }]
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
  const [docs, setDocs] = useState<DocumentMeta[]>([])
  const [loading, setLoading] = useState(true)
  const [uploading, setUploading] = useState(false)
  const [parsing, setParsing] = useState<number | null>(null)
  const [dragOver, setDragOver] = useState(false)
  const [cwd, setCwd] = useState("/")
  const [virtualFolders, setVirtualFolders] = useState<string[]>([])
  const [detailDoc, setDetailDoc] = useState<DocumentMeta | null>(null)
  const [search, setSearch] = useState("")
  const [moveDoc, setMoveDoc] = useState<DocumentMeta | null>(null)
  const [moveTarget, setMoveTarget] = useState("/")
  const [deleteDoc, setDeleteDoc] = useState<DocumentMeta | null>(null)
  const [extractDoc, setExtractDoc] = useState<DocumentMeta | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const versionRef = useRef<HTMLInputElement>(null)
  const versionDoc = useRef<DocumentMeta | null>(null)

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      setDocs(await api.listDocuments(ksId))
    } catch (e) {
      toast.error(`Failed to load documents: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [ksId])

  useEffect(() => { refresh() }, [refresh])

  const allFolders = useMemo(
    () => [...new Set([...docs.map((d) => d.folder), ...virtualFolders, "/"])],
    [docs, virtualFolders],
  )
  // When searching, flatten across ALL folders and hide subfolders; otherwise browse the tree.
  const searching = search.trim().length > 0
  const subfolders = useMemo(() => (searching ? [] : childFolders(allFolders, cwd)), [searching, allFolders, cwd])
  const docsHere = useMemo(
    () => (searching
      ? docs.filter((d) => d.original_filename.toLowerCase().includes(search.trim().toLowerCase()))
      : docs.filter((d) => d.folder === cwd)),
    [searching, search, docs, cwd],
  )

  const afterChange = useCallback(() => { refresh(); onChanged?.() }, [refresh, onChanged])

  const uploadFiles = useCallback(
    async (files: FileList | File[], folder: string) => {
      setUploading(true)
      let ok = 0
      for (const file of Array.from(files)) {
        try { await api.uploadDocument(ksId, file, folder); ok++ }
        catch (e) { toast.error(`${file.name} failed to upload: ${(e as Error).message}`) }
      }
      setUploading(false)
      if (ok) toast.success(`Uploaded ${ok} file(s) to ${folder}`)
      afterChange()
    },
    [ksId, afterChange],
  )

  const parse = useCallback(
    async (doc: DocumentMeta) => {
      setParsing(doc.id)
      try {
        const res = await api.parseDocument(ksId, doc.id)
        if (res.parse_status === "failed") toast.error(`Parse failed: ${res.error}`)
        else toast.success(`Parsed: ${res.chunk_count} chunks (${res.parser_backend})`)
        afterChange()
      } catch (e) {
        toast.error(`Parse error: ${(e as Error).message}`)
      } finally {
        setParsing(null)
      }
    },
    [ksId, afterChange],
  )

  const newFolder = () => {
    const name = prompt("New folder name")?.trim()
    if (!name) return
    const path = cwd === "/" ? "/" + name : cwd + "/" + name
    setVirtualFolders((v) => [...v, path])
    setCwd(path)
  }

  const doMove = useCallback(async () => {
    if (!moveDoc) return
    try {
      await api.moveDocument(ksId, moveDoc.id, moveTarget)
      toast.success("Moved")
      setMoveDoc(null)
      afterChange()
    } catch (e) {
      toast.error(`Failed to move: ${(e as Error).message}`)
    }
  }, [ksId, moveDoc, moveTarget, afterChange])

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

  const colSpan = 7

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="flex flex-wrap items-center gap-1 text-sm">
          {breadcrumbs(cwd).map((c, i) => (
            <span key={c.path} className="flex items-center gap-1">
              {i > 0 && <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />}
              <button
                onClick={() => setCwd(c.path)}
                className={i === breadcrumbs(cwd).length - 1 ? "font-medium" : "text-muted-foreground hover:text-foreground"}
              >
                {c.label}
              </button>
            </span>
          ))}
        </div>
        <div className="flex items-center gap-2">
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search all documents…" className="h-8 w-48 pl-7 text-sm" />
          </div>
          {canWrite && (
          <>
            <Button variant="outline" size="sm" onClick={newFolder}><FolderPlus className="h-4 w-4" /> New Folder</Button>
            <Button size="sm" onClick={() => inputRef.current?.click()} disabled={uploading}>
              {uploading ? <Loader2 className="h-4 w-4 animate-spin" /> : <Upload className="h-4 w-4" />} Upload here
            </Button>
          </>
          )}
        </div>
      </div>

      <input
        ref={inputRef} type="file" multiple className="hidden"
        accept=".pdf,.docx,.doc,.xlsx,.xls,.txt,.md,.csv"
        onChange={(e) => e.target.files && uploadFiles(e.target.files, cwd)}
      />
      <input
        ref={versionRef} type="file" className="hidden"
        accept=".pdf,.docx,.doc,.xlsx,.xls,.txt,.md,.csv"
        onChange={(e) => { onNewVersionPicked(e.target.files); e.target.value = "" }}
      />

      {canWrite && (
        <div
          onDragOver={(e) => { e.preventDefault(); setDragOver(true) }}
          onDragLeave={() => setDragOver(false)}
          onDrop={(e) => { e.preventDefault(); setDragOver(false); if (e.dataTransfer.files.length) uploadFiles(e.dataTransfer.files, cwd) }}
          onClick={() => inputRef.current?.click()}
          className={`flex cursor-pointer items-center justify-center gap-2 rounded-lg border-2 border-dashed p-4 text-sm transition-colors ${
            dragOver ? "border-primary bg-muted/50" : "border-border hover:border-primary/50"
          }`}
        >
          <Upload className="h-4 w-4 text-muted-foreground" />
          Drag & drop or click to upload to <span className="font-medium">{cwd}</span>
          <span className="text-xs text-muted-foreground">(PDF / Word / Excel / TXT / MD / CSV)</span>
        </div>
      )}

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Type</TableHead>
              <TableHead>Size</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Chunks</TableHead>
              <TableHead>Extraction</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {subfolders.map((f) => (
              <TableRow key={f} className="cursor-pointer" onClick={() => setCwd(f)}>
                <TableCell className="font-medium">
                  <span className="inline-flex items-center gap-2">
                    <Folder className="h-4 w-4 text-primary" />
                    {f.split("/").pop()}
                  </span>
                </TableCell>
                <TableCell className="text-muted-foreground">Folder</TableCell>
                <TableCell>—</TableCell><TableCell>—</TableCell><TableCell>—</TableCell><TableCell>—</TableCell>
                <TableCell className="text-right text-xs text-muted-foreground">Open</TableCell>
              </TableRow>
            ))}

            {loading ? (
              <TableRow><TableCell colSpan={colSpan} className="h-20 text-center text-muted-foreground">Loading…</TableCell></TableRow>
            ) : subfolders.length === 0 && docsHere.length === 0 ? (
              <TableRow><TableCell colSpan={colSpan} className="h-20 text-center text-muted-foreground">
                {searching
                  ? "No documents match your search."
                  : canWrite ? "This folder is empty. Upload files or create a subfolder." : "This folder is empty."}
              </TableCell></TableRow>
            ) : (
              docsHere.map((doc) => (
                <TableRow key={doc.id} className="cursor-pointer" onClick={() => setDetailDoc(doc)}>
                  <TableCell className="max-w-xs truncate font-medium">
                    <span className="inline-flex items-center gap-2">
                      <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
                      {doc.original_filename}
                    </span>
                  </TableCell>
                  <TableCell><Badge variant="outline" className="uppercase">{doc.ext}</Badge></TableCell>
                  <TableCell className="text-muted-foreground">{humanSize(doc.size_bytes)}</TableCell>
                  <TableCell><StatusBadge status={doc.parse_status} /></TableCell>
                  <TableCell className="text-muted-foreground">{doc.chunk_count || "—"}</TableCell>
                  <TableCell><ExtractionBadges doc={doc} /></TableCell>
                  <TableCell className="space-x-1 text-right" onClick={(e) => e.stopPropagation()}>
                    {canWrite && (
                      <Button size="sm" variant="outline" disabled={parsing === doc.id} onClick={() => parse(doc)}>
                        {parsing === doc.id && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
                        {doc.parse_status === "parsed" ? "Re-parse" : "Parse"}
                      </Button>
                    )}
                    {canWrite && doc.parse_status === "parsed" && doc.chunk_count > 0 && (
                      <Button size="sm" title="Extract schema / instances from this document" onClick={() => setExtractDoc(doc)}>
                        <Sparkles className="h-3.5 w-3.5" /> Extract
                      </Button>
                    )}
                    {canWrite && (
                      <>
                        <Button size="icon" variant="ghost" className="h-8 w-8" title="Upload new version" onClick={() => startNewVersion(doc)}>
                          <FileUp className="h-4 w-4" />
                        </Button>
                        <Button size="icon" variant="ghost" className="h-8 w-8" title="Move" onClick={() => { setMoveDoc(doc); setMoveTarget(doc.folder) }}>
                          <FolderInput className="h-4 w-4" />
                        </Button>
                        <Button size="icon" variant="ghost" className="h-8 w-8 text-muted-foreground hover:text-destructive" title="Delete" onClick={() => setDeleteDoc(doc)}>
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

      {/* Per-document detail (info, extraction history, contribution, chunks) */}
      {detailDoc && (
        <DocumentDetailSheet ksId={ksId} doc={detailDoc} onClose={() => setDetailDoc(null)} />
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

      {/* Move dialog */}
      <Dialog open={!!moveDoc} onOpenChange={(o) => !o && setMoveDoc(null)}>
        <DialogContent>
          <DialogHeader><DialogTitle>Move "{moveDoc?.original_filename}"</DialogTitle></DialogHeader>
          <div className="space-y-2 py-2">
            <Label>Target folder</Label>
            <FolderCombobox value={moveTarget} onChange={setMoveTarget} folders={allFolders} placeholder="/manuals/pumps" />
            <p className="text-xs text-muted-foreground">Paths that don't exist are created automatically (virtual folders).</p>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setMoveDoc(null)}>Cancel</Button>
            <Button onClick={doMove}>Move</Button>
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
