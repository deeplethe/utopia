import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { Check, Cpu, Loader2, Pencil, Plus, Sparkles, Star, Trash2, X, Zap } from "lucide-react"
import { api } from "@/lib/api"
import type { Provider, SystemSettings } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"

const err = (e: unknown) => (e as Error).message.replace(/^\d+:\s*/, "")

export default function SettingsPage() {
  const [providers, setProviders] = useState<Provider[]>([])
  const [s, setS] = useState<SystemSettings | null>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<number | null>(null)
  const [testStatus, setTestStatus] = useState<Record<number, "ok" | "fail">>({})
  const [editing, setEditing] = useState<Provider | "new" | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [ps, st] = await Promise.all([api.listProviders(), api.getSettings()])
      setProviders(ps)
      setS(st)
    } catch (e) {
      toast.error(`Failed to load: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [])
  useEffect(() => { load() }, [load])

  const setDefault = async (p: Provider) => {
    try {
      const st = p.kind === "embedding"
        ? await api.updateSettings({ embedding_provider_id: p.id })
        : await api.updateSettings({ llm_provider_id: p.id })
      setS(st)
      toast.success(`Default ${p.kind} → ${p.name}`)
    } catch (e) {
      toast.error(err(e))
    }
  }

  const del = async (p: Provider) => {
    if (!confirm(`Delete "${p.name}"?`)) return
    try {
      await api.deleteProvider(p.id)
      toast.success("Deleted")
      load()
    } catch (e) {
      toast.error(err(e))
    }
  }

  const testRow = async (p: Provider) => {
    setBusy(p.id)
    const id = toast.loading(`Testing ${p.name}…`)
    try {
      const r = await api.testProvider({ provider_id: p.id })
      setTestStatus((m) => ({ ...m, [p.id]: r.ok ? "ok" : "fail" }))
      if (r.ok) toast.success(r.message, { id })
      else toast.error(r.message, { id })
    } catch (e) {
      setTestStatus((m) => ({ ...m, [p.id]: "fail" }))
      toast.error(err(e), { id })
    } finally {
      setBusy(null)
    }
  }

  if (loading || !s) {
    return (
      <div className="flex h-40 items-center justify-center text-muted-foreground">
        {loading ? <Loader2 className="h-5 w-5 animate-spin" /> : "No settings"}
      </div>
    )
  }

  const isDefault = (p: Provider) =>
    p.kind === "embedding" ? s.embedding_provider_id === p.id : s.llm_provider_id === p.id
  // Current-session test result wins; otherwise fall back to the persisted last-test result.
  const rowStatus = (p: Provider): "ok" | "fail" | undefined =>
    testStatus[p.id] ?? (p.last_test_ok == null ? undefined : p.last_test_ok ? "ok" : "fail")

  return (
    <div className="max-w-4xl space-y-5">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Model Endpoints</h1>
          <p className="text-sm text-muted-foreground">
            Each row is one endpoint + key + model. Star one <b>LLM</b> and one <b>embedding</b> as the
            default; a knowledge system can point at any. Temperature {s.temperature} (backend/.env).
          </p>
        </div>
        <Button size="sm" onClick={() => setEditing("new")}><Plus className="h-4 w-4" /> Add model</Button>
      </div>

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-32">Type</TableHead>
              <TableHead>Name</TableHead>
              <TableHead>Model</TableHead>
              <TableHead>Endpoint</TableHead>
              <TableHead className="w-24">Key</TableHead>
              <TableHead className="w-44 text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {providers.length === 0 ? (
              <TableRow><TableCell colSpan={6} className="h-16 text-center text-muted-foreground">No model entries — click "Add model".</TableCell></TableRow>
            ) : providers.map((p) => (
              <TableRow key={p.id}>
                <TableCell>
                  <span className="flex items-center gap-1.5">
                    <Badge variant="outline" className="gap-1 text-[10px]">
                      {p.kind === "embedding" ? <Cpu className="h-3 w-3" /> : <Sparkles className="h-3 w-3 text-primary" />}
                      {p.kind}
                    </Badge>
                    {isDefault(p) && <Badge className="gap-1 text-[10px]"><Star className="h-3 w-3" /> default</Badge>}
                  </span>
                </TableCell>
                <TableCell className="font-medium">{p.name}</TableCell>
                <TableCell className="font-mono text-xs">{p.model || <span className="text-muted-foreground">—</span>}</TableCell>
                <TableCell className="max-w-[14rem] truncate text-xs text-muted-foreground" title={p.base_url}>{p.base_url}</TableCell>
                <TableCell className="font-mono text-xs">{p.has_api_key ? p.api_key_hint : <span className="text-muted-foreground">—</span>}</TableCell>
                <TableCell className="space-x-1 text-right">
                  <Button size="sm" variant="ghost" className="h-7" disabled={busy === p.id} onClick={() => testRow(p)}>
                    {busy === p.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      : rowStatus(p) === "ok" ? <Check className="h-3.5 w-3.5 text-emerald-500" />
                      : rowStatus(p) === "fail" ? <X className="h-3.5 w-3.5 text-destructive" />
                      : <Zap className="h-3.5 w-3.5" />} Test
                  </Button>
                  {!isDefault(p) && (
                    <Button size="icon" variant="ghost" className="h-7 w-7" title="Set as default" onClick={() => setDefault(p)}>
                      <Star className="h-3.5 w-3.5" />
                    </Button>
                  )}
                  <Button size="icon" variant="ghost" className="h-7 w-7" title="Edit" onClick={() => setEditing(p)}>
                    <Pencil className="h-3.5 w-3.5" />
                  </Button>
                  <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive" title="Delete" onClick={() => del(p)}>
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      {editing && (
        <ModelDialog
          entry={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => { setEditing(null); load() }}
        />
      )}
    </div>
  )
}

function ModelDialog({ entry, onClose, onSaved }: { entry: Provider | null; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(entry?.name ?? "")
  const [kind, setKind] = useState<"llm" | "embedding">(entry?.kind ?? "llm")
  const [baseUrl, setBaseUrl] = useState(entry?.base_url ?? "https://openrouter.ai/api/v1")
  const [apiKey, setApiKey] = useState("")
  const [model, setModel] = useState(entry?.model ?? "")
  const [saving, setSaving] = useState(false)
  const [testing, setTesting] = useState(false)
  const [testOk, setTestOk] = useState<boolean | null>(null)

  const test = async () => {
    setTesting(true)
    try {
      const r = await api.testProvider({
        provider_id: entry?.id, base_url: baseUrl.trim(), api_key: apiKey.trim() || undefined,
        model: model.trim(), kind,
      })
      setTestOk(r.ok)
      if (r.ok) toast.success(r.message)
      else toast.error(r.message)
    } catch (e) {
      setTestOk(false)
      toast.error(err(e))
    } finally {
      setTesting(false)
    }
  }

  const save = async () => {
    if (!name.trim() || !model.trim()) return
    setSaving(true)
    try {
      if (entry) {
        await api.updateProvider(entry.id, { name: name.trim(), kind, base_url: baseUrl.trim(), model: model.trim(), api_key: apiKey.trim() || undefined })
      } else {
        await api.createProvider({ name: name.trim(), kind, base_url: baseUrl.trim(), model: model.trim(), api_key: apiKey.trim() })
      }
      toast.success("Saved")
      onSaved()
    } catch (e) {
      toast.error(err(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader><DialogTitle>{entry ? `Edit "${entry.name}"` : "Add model endpoint"}</DialogTitle></DialogHeader>
        <div className="space-y-4 py-2">
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="m-name">Name</Label>
              <Input id="m-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. OpenRouter DeepSeek" />
            </div>
            <div className="space-y-1.5">
              <Label>Type</Label>
              <Select value={kind} onValueChange={(v) => setKind(v as "llm" | "embedding")}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="llm">LLM (chat)</SelectItem>
                  <SelectItem value="embedding">Embedding</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="m-url">API base URL</Label>
            <Input id="m-url" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://openrouter.ai/api/v1" spellCheck={false} />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="m-key">API key (sk-…)</Label>
            <Input id="m-key" type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)}
              placeholder={entry?.has_api_key ? `Leave blank to keep (${entry.api_key_hint})` : "sk-…"}
              autoComplete="off" spellCheck={false} />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="m-model">Model name</Label>
            <Input id="m-model" value={model} onChange={(e) => setModel(e.target.value)}
              placeholder={kind === "embedding" ? "baai/bge-m3" : "deepseek/deepseek-chat"} spellCheck={false} />
          </div>
        </div>
        <DialogFooter className="sm:justify-between">
          <Button variant="outline" onClick={test} disabled={testing || !model.trim()}>
            {testing ? <Loader2 className="h-4 w-4 animate-spin" />
              : testOk === true ? <Check className="h-4 w-4 text-emerald-500" />
              : testOk === false ? <X className="h-4 w-4 text-destructive" />
              : <Zap className="h-4 w-4" />} Test
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" onClick={onClose}>Cancel</Button>
            <Button onClick={save} disabled={saving || !name.trim() || !model.trim()}>
              {saving && <Loader2 className="h-4 w-4 animate-spin" />} Save
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
