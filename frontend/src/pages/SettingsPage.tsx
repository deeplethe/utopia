import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { Check, Cpu, Loader2, Pencil, Plus, Sparkles, Star, Trash2, X, Zap } from "lucide-react"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
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
  const { t } = useI18n()
  const confirmAction = useConfirm()
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
      toast.error(t("common.failedLoad", { error: (e as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [t])
  useEffect(() => { load() }, [load])

  const setDefault = async (p: Provider) => {
    try {
      const st = p.kind === "embedding"
        ? await api.updateSettings({ embedding_provider_id: p.id })
        : await api.updateSettings({ llm_provider_id: p.id })
      setS(st)
      toast.success(t("models.defaultChanged", { kind: p.kind, name: p.name }))
    } catch (e) {
      toast.error(err(e))
    }
  }

  const del = async (p: Provider) => {
    if (!await confirmAction(t("models.deleteConfirm", { name: p.name }), { destructive: true })) return
    try {
      await api.deleteProvider(p.id)
      toast.success(t("common.deleted"))
      load()
    } catch (e) {
      toast.error(err(e))
    }
  }

  const testRow = async (p: Provider) => {
    setBusy(p.id)
    const id = toast.loading(t("models.testing", { name: p.name }))
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
        {loading ? <Loader2 className="h-5 w-5 animate-spin" /> : t("models.noSettings")}
      </div>
    )
  }

  const isDefault = (p: Provider) =>
    p.kind === "embedding" ? s.embedding_provider_id === p.id : s.llm_provider_id === p.id
  // Current-session test result wins; otherwise fall back to the persisted last-test result.
  const rowStatus = (p: Provider): "ok" | "fail" | undefined =>
    testStatus[p.id] ?? (p.last_test_ok == null ? undefined : p.last_test_ok ? "ok" : "fail")

  return (
    <div className="max-w-5xl space-y-5">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">{t("models.title")}</h1>
          <p className="mt-1 text-sm text-muted-foreground">{t("models.description")}</p>
        </div>
        <Button size="sm" onClick={() => setEditing("new")}><Plus className="h-4 w-4" /> {t("models.add")}</Button>
      </div>

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-32">{t("common.type")}</TableHead>
              <TableHead>{t("common.name")}</TableHead>
              <TableHead>{t("models.model")}</TableHead>
              <TableHead>{t("models.endpoint")}</TableHead>
              <TableHead className="w-24">{t("models.key")}</TableHead>
              <TableHead className="w-24 text-center">{t("models.concurrencyLimit")}</TableHead>
              <TableHead className="w-44 text-right">{t("common.actions")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {providers.length === 0 ? (
              <TableRow><TableCell colSpan={7} className="h-16 text-center text-muted-foreground">{t("models.empty")}</TableCell></TableRow>
            ) : providers.map((p) => (
              <TableRow key={p.id}>
                <TableCell>
                  <span className="flex items-center gap-1.5">
                    <Badge variant="outline" className="gap-1 text-[10px]">
                      {p.kind === "embedding" ? <Cpu className="h-3 w-3" /> : <Sparkles className="h-3 w-3 text-primary" />}
                      {p.kind}
                    </Badge>
                    {isDefault(p) && <Badge className="gap-1 text-[10px]"><Star className="h-3 w-3" /> {t("models.default")}</Badge>}
                  </span>
                </TableCell>
                <TableCell className="font-medium">{p.name}</TableCell>
                <TableCell className="font-mono text-xs">{p.model || <span className="text-muted-foreground">—</span>}</TableCell>
                <TableCell className="max-w-[14rem] truncate text-xs text-muted-foreground" title={p.base_url}>{p.base_url}</TableCell>
                <TableCell className="font-mono text-xs">{p.has_api_key ? p.api_key_hint : <span className="text-muted-foreground">—</span>}</TableCell>
                <TableCell className="text-center font-mono text-sm">{p.concurrency_limit}</TableCell>
                <TableCell className="space-x-1 text-right">
                  <Button size="sm" variant="ghost" className="h-7" disabled={busy === p.id} onClick={() => testRow(p)}>
                    {busy === p.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      : rowStatus(p) === "ok" ? <Check className="h-3.5 w-3.5 text-emerald-500" />
                      : rowStatus(p) === "fail" ? <X className="h-3.5 w-3.5 text-destructive" />
                      : <Zap className="h-3.5 w-3.5" />} {t("common.test")}
                  </Button>
                  {!isDefault(p) && (
                    <Button size="icon" variant="ghost" className="h-7 w-7" title={t("models.setDefault")} onClick={() => setDefault(p)}>
                      <Star className="h-3.5 w-3.5" />
                    </Button>
                  )}
                  <Button size="icon" variant="ghost" className="h-7 w-7" title={t("common.edit")} onClick={() => setEditing(p)}>
                    <Pencil className="h-3.5 w-3.5" />
                  </Button>
                  <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive" title={t("common.delete")} onClick={() => del(p)}>
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
  const { t } = useI18n()
  const [name, setName] = useState(entry?.name ?? "")
  const [kind, setKind] = useState<"llm" | "embedding">(entry?.kind ?? "llm")
  const [baseUrl, setBaseUrl] = useState(entry?.base_url ?? "https://openrouter.ai/api/v1")
  const [apiKey, setApiKey] = useState("")
  const [model, setModel] = useState(entry?.model ?? "")
  const [concurrencyLimit, setConcurrencyLimit] = useState(String(entry?.concurrency_limit ?? 10))
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
    const limit = Number(concurrencyLimit)
    if (!Number.isInteger(limit) || limit < 1 || limit > 64) {
      toast.error(t("models.concurrencyRange"))
      return
    }
    setSaving(true)
    try {
      if (entry) {
        await api.updateProvider(entry.id, {
          name: name.trim(), kind, base_url: baseUrl.trim(), model: model.trim(),
          api_key: apiKey.trim() || undefined, concurrency_limit: limit,
        })
      } else {
        await api.createProvider({
          name: name.trim(), kind, base_url: baseUrl.trim(), model: model.trim(),
          api_key: apiKey.trim(), concurrency_limit: limit,
        })
      }
      toast.success(t("common.saved"))
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
        <DialogHeader><DialogTitle>{entry ? t("models.editTitle", { name: entry.name }) : t("models.addTitle")}</DialogTitle></DialogHeader>
        <div className="space-y-4 py-2">
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="m-name">{t("common.name")}</Label>
              <Input id="m-name" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("models.namePlaceholder")} />
            </div>
            <div className="space-y-1.5">
              <Label>{t("common.type")}</Label>
              <Select value={kind} onValueChange={(v) => setKind(v as "llm" | "embedding")}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="llm">{t("models.llmChat")}</SelectItem>
                  <SelectItem value="embedding">{t("models.embedding")}</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="m-url">{t("models.apiBaseUrl")}</Label>
            <Input id="m-url" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://openrouter.ai/api/v1" spellCheck={false} />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="m-key">{t("models.apiKey")}</Label>
            <Input id="m-key" type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)}
              placeholder={entry?.has_api_key ? t("models.keepKey", { hint: entry.api_key_hint ?? "" }) : "sk-…"}
              autoComplete="off" spellCheck={false} />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="m-model">{t("models.modelName")}</Label>
            <Input id="m-model" value={model} onChange={(e) => setModel(e.target.value)}
              placeholder={kind === "embedding" ? "baai/bge-m3" : "deepseek/deepseek-chat"} spellCheck={false} />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="m-concurrency">{t("models.concurrencyLimit")}</Label>
            <Input
              id="m-concurrency"
              type="number"
              min={1}
              max={64}
              step={1}
              value={concurrencyLimit}
              onChange={(e) => setConcurrencyLimit(e.target.value)}
            />
            <p className="text-xs text-muted-foreground">{t("models.concurrencyLimitHelp")}</p>
          </div>
        </div>
        <DialogFooter className="sm:justify-between">
          <Button variant="outline" onClick={test} disabled={testing || !model.trim()}>
            {testing ? <Loader2 className="h-4 w-4 animate-spin" />
              : testOk === true ? <Check className="h-4 w-4 text-emerald-500" />
              : testOk === false ? <X className="h-4 w-4 text-destructive" />
              : <Zap className="h-4 w-4" />} {t("common.test")}
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" onClick={onClose}>{t("common.cancel")}</Button>
            <Button onClick={save} disabled={saving || !name.trim() || !model.trim()}>
              {saving && <Loader2 className="h-4 w-4 animate-spin" />} {t("common.save")}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
