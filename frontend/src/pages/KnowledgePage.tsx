import type { MouseEvent, ReactNode } from "react"
import { useCallback, useEffect, useState } from "react"
import { useNavigate } from "react-router-dom"
import { toast } from "sonner"
import { Boxes, Link2, Network, Pencil, Plus, Trash2 } from "lucide-react"
import { api } from "@/lib/api"
import type { KnowledgeSystem, Provider, Role } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"

const ROLE_LABEL: Record<Role, string> = { owner: "Owner", editor: "Editor", viewer: "Viewer" }
const SYS = "0" // Select sentinel for "use the system default" (provider ids are >= 1)

function Stat({ icon, value, label }: { icon: ReactNode; value: number; label: string }) {
  return (
    <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
      {icon}
      <span className="font-medium text-foreground">{value}</span>
      {label}
    </div>
  )
}

export default function KnowledgePage() {
  const [systems, setSystems] = useState<KnowledgeSystem[]>([])
  const [loading, setLoading] = useState(true)
  const [open, setOpen] = useState(false)
  const [name, setName] = useState("")
  const [desc, setDesc] = useState("")
  const [providers, setProviders] = useState<Provider[]>([])
  const [llmProv, setLlmProv] = useState(SYS)
  const [embProv, setEmbProv] = useState(SYS)
  const [creating, setCreating] = useState(false)
  // Edit-existing-KS dialog state.
  const [editKS, setEditKS] = useState<KnowledgeSystem | null>(null)
  const [editName, setEditName] = useState("")
  const [editDesc, setEditDesc] = useState("")
  const [editLlmProv, setEditLlmProv] = useState(SYS)
  const [editEmbProv, setEditEmbProv] = useState(SYS)
  const [savingEdit, setSavingEdit] = useState(false)
  const navigate = useNavigate()

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      setSystems(await api.listKS())
    } catch (e) {
      toast.error(`Failed to load: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    refresh()
  }, [refresh])

  useEffect(() => {
    api.listProviders().then(setProviders).catch(() => {})
  }, [])

  const create = useCallback(async () => {
    if (!name.trim()) return
    setCreating(true)
    try {
      const ks = await api.createKS(name.trim(), desc.trim(), {
        llm_provider_id: Number(llmProv),         // 0 => system default
        embedding_provider_id: Number(embProv),
      })
      toast.success(`Created knowledge system "${ks.name}"`)
      setOpen(false)
      setName("")
      setDesc("")
      setLlmProv(SYS)
      setEmbProv(SYS)
      navigate(`/knowledge/${ks.id}`)
    } catch (e) {
      toast.error(`Failed to create: ${(e as Error).message}`)
    } finally {
      setCreating(false)
    }
  }, [name, desc, llmProv, embProv, navigate])

  const remove = useCallback(
    async (ks: KnowledgeSystem, e: MouseEvent) => {
      e.stopPropagation()
      if (!confirm(`Delete knowledge system "${ks.name}" and its ontology graph? This cannot be undone.`)) return
      try {
        await api.deleteKS(ks.id)
        toast.success("Deleted")
        refresh()
      } catch (err) {
        toast.error(`Failed to delete: ${(err as Error).message}`)
      }
    },
    [refresh],
  )

  const openEdit = useCallback((ks: KnowledgeSystem, e: MouseEvent) => {
    e.stopPropagation()
    setEditKS(ks)
    setEditName(ks.name)
    setEditDesc(ks.description)
    setEditLlmProv(ks.llm_provider_id ? String(ks.llm_provider_id) : SYS)
    setEditEmbProv(ks.embedding_provider_id ? String(ks.embedding_provider_id) : SYS)
  }, [])

  const saveEdit = useCallback(async () => {
    if (!editKS || !editName.trim()) return
    setSavingEdit(true)
    try {
      await api.updateKS(editKS.id, {
        name: editName.trim(),
        description: editDesc,
        llm_provider_id: Number(editLlmProv),        // 0 => clear to system default
        embedding_provider_id: Number(editEmbProv),
      })
      toast.success("Saved")
      setEditKS(null)
      refresh()
    } catch (e) {
      toast.error(`Failed to save: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    } finally {
      setSavingEdit(false)
    }
  }, [editKS, editName, editDesc, editLlmProv, editEmbProv, refresh])

  // Two entry pickers (LLM + embedding), each defaulting to the system default. Shared by both dialogs.
  const provPicker = (kind: "llm" | "embedding", value: string, onChange: (v: string) => void) => (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem value={SYS}>System default</SelectItem>
        {providers.filter((p) => p.kind === kind).map((p) => (
          <SelectItem key={p.id} value={String(p.id)}>{p.name} · {p.model}</SelectItem>
        ))}
      </SelectContent>
    </Select>
  )

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Knowledge Systems</h1>
          <p className="text-sm text-muted-foreground">Each knowledge system is an independent ontology graph (TBox).</p>
        </div>
        <Dialog open={open} onOpenChange={setOpen}>
          <DialogTrigger asChild>
            <Button>
              <Plus className="h-4 w-4" />
              New Knowledge System
            </Button>
          </DialogTrigger>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>New Knowledge System</DialogTitle>
            </DialogHeader>
            <div className="space-y-4 py-2">
              <div className="space-y-2">
                <Label htmlFor="ks-name">Name</Label>
                <Input
                  id="ks-name"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="e.g. Pump station equipment ontology"
                  onKeyDown={(e) => e.key === "Enter" && create()}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="ks-desc">Description (optional)</Label>
                <Textarea
                  id="ks-desc"
                  value={desc}
                  onChange={(e) => setDesc(e.target.value)}
                  placeholder="What this ontology is for, its domain scope…"
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-2">
                  <Label>LLM</Label>
                  {provPicker("llm", llmProv, setLlmProv)}
                </div>
                <div className="space-y-2">
                  <Label>Embedding</Label>
                  {provPicker("embedding", embProv, setEmbProv)}
                </div>
              </div>
              <p className="text-xs text-muted-foreground">Optional — override the default models for this knowledge system. Manage entries in Settings.</p>
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={() => setOpen(false)}>Cancel</Button>
              <Button onClick={create} disabled={creating || !name.trim()}>Create</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>

      {/* Edit an existing KS: name / description / per-KS model */}
      <Dialog open={!!editKS} onOpenChange={(o) => !o && setEditKS(null)}>
        <DialogContent>
          <DialogHeader><DialogTitle>Edit "{editKS?.name}"</DialogTitle></DialogHeader>
          <div className="space-y-4 py-2">
            <div className="space-y-2">
              <Label htmlFor="edit-ks-name">Name</Label>
              <Input id="edit-ks-name" value={editName} onChange={(e) => setEditName(e.target.value)} />
            </div>
            <div className="space-y-2">
              <Label htmlFor="edit-ks-desc">Description</Label>
              <Textarea id="edit-ks-desc" value={editDesc} onChange={(e) => setEditDesc(e.target.value)} />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-2">
                <Label>LLM</Label>
                {provPicker("llm", editLlmProv, setEditLlmProv)}
              </div>
              <div className="space-y-2">
                <Label>Embedding</Label>
                {provPicker("embedding", editEmbProv, setEditEmbProv)}
              </div>
            </div>
            <p className="text-xs text-muted-foreground">Override the default models for this knowledge system.</p>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setEditKS(null)}>Cancel</Button>
            <Button onClick={saveEdit} disabled={savingEdit || !editName.trim()}>Save</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {loading ? (
        <p className="text-sm text-muted-foreground">Loading…</p>
      ) : systems.length === 0 ? (
        <div className="rounded-lg border border-dashed p-12 text-center">
          <Network className="mx-auto mb-3 h-8 w-8 text-muted-foreground" />
          <p className="text-sm text-muted-foreground">No knowledge systems yet. Click "New" at the top right to create your first ontology.</p>
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {systems.map((ks) => (
            <Card
              key={ks.id}
              className="cursor-pointer transition-colors hover:border-primary/50"
              onClick={() => navigate(`/knowledge/${ks.id}`)}
            >
              <CardHeader>
                <div className="flex items-start justify-between gap-2">
                  <CardTitle className="text-base">{ks.name}</CardTitle>
                  <div className="flex shrink-0 items-center gap-1">
                    <Badge variant="outline" className="text-[10px]">{ROLE_LABEL[ks.my_role]}</Badge>
                    {ks.my_role !== "viewer" && (
                      <Button
                        size="icon"
                        variant="ghost"
                        className="h-7 w-7 text-muted-foreground hover:text-foreground"
                        title="Edit settings"
                        onClick={(e) => openEdit(ks, e)}
                      >
                        <Pencil className="h-3.5 w-3.5" />
                      </Button>
                    )}
                    {ks.my_role === "owner" && (
                      <Button
                        size="icon"
                        variant="ghost"
                        className="h-7 w-7 text-muted-foreground hover:text-destructive"
                        onClick={(e) => remove(ks, e)}
                      >
                        <Trash2 className="h-4 w-4" />
                      </Button>
                    )}
                  </div>
                </div>
                <CardDescription className="line-clamp-2 min-h-[2.5rem]">
                  {ks.description || "(no description)"}
                </CardDescription>
              </CardHeader>
              <CardContent className="flex gap-4">
                <Stat icon={<Boxes className="h-3.5 w-3.5" />} value={ks.class_count} label="classes" />
                <Stat icon={<Link2 className="h-3.5 w-3.5" />} value={ks.property_count} label="properties" />
                <Stat icon={<Network className="h-3.5 w-3.5" />} value={ks.axiom_count} label="axioms" />
              </CardContent>
              <CardFooter className="text-xs text-muted-foreground">
                {new Date(ks.created_at).toLocaleString()}
              </CardFooter>
            </Card>
          ))}
        </div>
      )}
    </div>
  )
}
