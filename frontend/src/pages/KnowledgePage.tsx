import type { MouseEvent, ReactNode } from "react"
import { useCallback, useEffect, useState } from "react"
import { useNavigate } from "react-router-dom"
import { toast } from "sonner"
import { Boxes, Cpu, FileUp, Link2, Network, Pencil, Plus, Sparkles, Trash2 } from "lucide-react"
import { api } from "@/lib/api"
import type { KnowledgeSystem, Provider, Role } from "@/lib/types"
import { useI18n, type MessageKey } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
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

const ROLE_LABEL: Record<Role, MessageKey> = {
  owner: "common.owner",
  editor: "common.editor",
  viewer: "common.viewer",
}
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
  const { locale, t } = useI18n()
  const confirmAction = useConfirm()
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
      toast.error(t("common.failedLoad", { error: (e as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [t])

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
      toast.success(t("knowledge.created", { name: ks.name }))
      setOpen(false)
      setName("")
      setDesc("")
      setLlmProv(SYS)
      setEmbProv(SYS)
      navigate(`/knowledge/${ks.id}`)
    } catch (e) {
      toast.error(t("common.failedCreate", { error: (e as Error).message }))
    } finally {
      setCreating(false)
    }
  }, [name, desc, llmProv, embProv, navigate, t])

  const remove = useCallback(
    async (ks: KnowledgeSystem, e: MouseEvent) => {
      e.stopPropagation()
      if (!await confirmAction(t("knowledge.deleteConfirm", { name: ks.name }), { destructive: true })) return
      try {
        await api.deleteKS(ks.id)
        toast.success(t("common.deleted"))
        refresh()
      } catch (err) {
        toast.error(t("common.failedDelete", { error: (err as Error).message }))
      }
    },
    [confirmAction, refresh, t],
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
      toast.success(t("common.saved"))
      setEditKS(null)
      refresh()
    } catch (e) {
      toast.error(t("common.failedSave", { error: (e as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setSavingEdit(false)
    }
  }, [editKS, editName, editDesc, editLlmProv, editEmbProv, refresh, t])

  // Two entry pickers (LLM + embedding), each defaulting to the system default. Shared by both dialogs.
  const provPicker = (kind: "llm" | "embedding", value: string, onChange: (v: string) => void) => (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem value={SYS}>{t("common.systemDefault")}</SelectItem>
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
          <h1 className="text-xl font-semibold tracking-tight">{t("knowledge.title")}</h1>
        </div>
        <Dialog open={open} onOpenChange={setOpen}>
          <DialogTrigger asChild>
            <Button>
              <Plus className="h-4 w-4" />
              {t("knowledge.new")}
            </Button>
          </DialogTrigger>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{t("knowledge.new")}</DialogTitle>
            </DialogHeader>
            <div className="space-y-4 py-2">
              <div className="space-y-2">
                <Label htmlFor="ks-name">{t("common.name")}</Label>
                <Input
                  id="ks-name"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={t("knowledge.namePlaceholder")}
                  onKeyDown={(e) => e.key === "Enter" && create()}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="ks-desc">{t("knowledge.descriptionOptional")}</Label>
                <Textarea
                  id="ks-desc"
                  value={desc}
                  onChange={(e) => setDesc(e.target.value)}
                  placeholder={t("knowledge.descriptionPlaceholder")}
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
              <p className="text-xs text-muted-foreground">{t("knowledge.modelOverrideNote")}</p>
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={() => setOpen(false)}>{t("common.cancel")}</Button>
              <Button onClick={create} disabled={creating || !name.trim()}>{t("common.create")}</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>

      {/* Edit an existing KS: name / description / per-KS model */}
      <Dialog open={!!editKS} onOpenChange={(o) => !o && setEditKS(null)}>
        <DialogContent>
          <DialogHeader><DialogTitle>{t("knowledge.edit", { name: editKS?.name ?? "" })}</DialogTitle></DialogHeader>
          <div className="space-y-4 py-2">
            <div className="space-y-2">
              <Label htmlFor="edit-ks-name">{t("common.name")}</Label>
              <Input id="edit-ks-name" value={editName} onChange={(e) => setEditName(e.target.value)} />
            </div>
            <div className="space-y-2">
              <Label htmlFor="edit-ks-desc">{t("common.description")}</Label>
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
            <p className="text-xs text-muted-foreground">{t("knowledge.modelOverrideShort")}</p>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setEditKS(null)}>{t("common.cancel")}</Button>
            <Button onClick={saveEdit} disabled={savingEdit || !editName.trim()}>{t("common.save")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {loading ? (
        <p className="text-sm text-muted-foreground">{t("common.loading")}</p>
      ) : systems.length === 0 ? (
        <div className="rounded-xl border p-6 md:p-8">
          <div className="mx-auto max-w-3xl space-y-6">
            <div className="text-center">
              <Sparkles className="mx-auto mb-3 h-8 w-8 text-primary" />
              <h2 className="text-lg font-semibold">{t("setup.title")}</h2>
              <p className="mt-1 text-sm text-muted-foreground">{t("setup.description")}</p>
            </div>
            <div className="grid gap-3 md:grid-cols-3">
              <div className="rounded-lg border p-4">
                <Cpu className="mb-3 h-5 w-5" />
                <p className="text-sm font-medium">1. {t("setup.models")}</p>
                <p className="mt-1 text-xs text-muted-foreground">{t("setup.modelsDescription")}</p>
                <Button className="mt-4" size="sm" variant="outline" onClick={() => navigate("/settings/models")}>{t("setup.configure")}</Button>
              </div>
              <div className="rounded-lg border p-4">
                <Network className="mb-3 h-5 w-5" />
                <p className="text-sm font-medium">2. {t("setup.knowledge")}</p>
                <p className="mt-1 text-xs text-muted-foreground">{t("setup.knowledgeDescription")}</p>
                <Button className="mt-4" size="sm" onClick={() => setOpen(true)}>{t("knowledge.new")}</Button>
              </div>
              <div className="rounded-lg border p-4">
                <FileUp className="mb-3 h-5 w-5" />
                <p className="text-sm font-medium">3. {t("setup.workflow")}</p>
                <p className="mt-1 text-xs text-muted-foreground">{t("setup.workflowDescription")}</p>
              </div>
            </div>
          </div>
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
                    <Badge variant="outline" className="text-[10px]">{t(ROLE_LABEL[ks.my_role])}</Badge>
                    {ks.my_role !== "viewer" && (
                      <Button
                        size="icon"
                        variant="ghost"
                        className="h-7 w-7 text-muted-foreground hover:text-foreground"
                        title={t("knowledge.editSettings")}
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
                  {ks.description || t("common.noDescription")}
                </CardDescription>
              </CardHeader>
              <CardContent className="flex gap-4">
                <Stat icon={<Boxes className="h-3.5 w-3.5" />} value={ks.class_count} label={t("knowledge.classes")} />
                <Stat icon={<Link2 className="h-3.5 w-3.5" />} value={ks.property_count} label={t("knowledge.properties")} />
                <Stat icon={<Network className="h-3.5 w-3.5" />} value={ks.axiom_count} label={t("knowledge.axioms")} />
              </CardContent>
              <CardFooter className="text-xs text-muted-foreground">
                {new Date(ks.created_at).toLocaleString(locale)}
              </CardFooter>
            </Card>
          ))}
        </div>
      )}
    </div>
  )
}
