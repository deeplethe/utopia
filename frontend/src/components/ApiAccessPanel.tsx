import { useCallback, useEffect, useMemo, useState } from "react"
import { Check, Copy, Eye, KeyRound, Loader2, Plus, ShieldCheck, Trash2 } from "lucide-react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type { ApiToken, ApiTokenCreated, ApiTokenScope, KnowledgeSystem } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"

const DEFAULT_SCOPES = new Set<ApiTokenScope>(["ontology:read", "vocabulary:read", "instances:read", "query:read"])

function when(value: string | null, locale: string, never: string) {
  return value ? new Date(value).toLocaleString(locale) : never
}

function StatusBadge({ status }: { status: ApiToken["status"] }) {
  const { t } = useI18n()
  if (status === "active") return <Badge className="bg-emerald-600 hover:bg-emerald-600">{t("api.status.active")}</Badge>
  return <Badge variant="secondary">{status === "expired" ? t("api.status.expired") : t("api.status.revoked")}</Badge>
}

export default function ApiAccessPanel({
  ks, canManage,
}: {
  ks: KnowledgeSystem
  canManage: boolean
}) {
  const { locale, t } = useI18n()
  const confirmAction = useConfirm()
  const scopeOptions: { value: ApiTokenScope; label: string; description: string }[] = [
    { value: "ontology:read", label: t("api.scope.ontology"), description: t("api.scope.ontologyDescription") },
    { value: "vocabulary:read", label: t("api.scope.vocabulary"), description: t("api.scope.vocabularyDescription") },
    { value: "instances:read", label: t("api.scope.instances"), description: t("api.scope.instancesDescription") },
    { value: "query:read", label: t("api.scope.query"), description: t("api.scope.queryDescription") },
    { value: "provenance:read", label: t("api.scope.provenance"), description: t("api.scope.provenanceDescription") },
  ]
  const [tokens, setTokens] = useState<ApiToken[]>([])
  const [loading, setLoading] = useState(true)
  const [createOpen, setCreateOpen] = useState(false)
  const [creating, setCreating] = useState(false)
  const [name, setName] = useState("")
  const [expiry, setExpiry] = useState("90")
  const [scopes, setScopes] = useState<Set<ApiTokenScope>>(new Set(DEFAULT_SCOPES))
  const [created, setCreated] = useState<ApiTokenCreated | null>(null)
  const [revealed, setRevealed] = useState<{ row: ApiToken; token: string } | null>(null)
  const [revealingId, setRevealingId] = useState<number | null>(null)
  const baseUrl = useMemo(
    () => `${window.location.origin}/api/v1/knowledge-systems/${ks.public_id}`,
    [ks.public_id],
  )

  const load = useCallback(async () => {
    if (!canManage) return
    setLoading(true)
    try {
      setTokens(await api.listApiTokens(ks.id))
    } catch (error) {
      toast.error(t("api.loadFailed", { error: (error as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [canManage, ks.id, t])

  useEffect(() => { load() }, [load])

  const openCreate = () => {
    setName("")
    setExpiry("90")
    setScopes(new Set(DEFAULT_SCOPES))
    setCreateOpen(true)
  }

  const toggleScope = (scope: ApiTokenScope) => {
    setScopes((current) => {
      const next = new Set(current)
      if (next.has(scope)) {
        next.delete(scope)
        if (scope === "instances:read") next.delete("provenance:read")
      } else {
        next.add(scope)
        if (scope === "provenance:read") next.add("instances:read")
      }
      return next
    })
  }

  const create = async () => {
    if (!name.trim() || scopes.size === 0) return
    setCreating(true)
    try {
      const result = await api.createApiToken(ks.id, {
        name: name.trim(),
        scopes: [...scopes],
        expires_in_days: expiry === "never" ? null : Number(expiry),
      })
      setCreateOpen(false)
      setCreated(result)
      await load()
    } catch (error) {
      toast.error(t("api.createFailed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setCreating(false)
    }
  }

  const revoke = async (token: ApiToken) => {
    if (!await confirmAction(t("api.revokeConfirm", { name: token.name }), { destructive: true })) return
    try {
      await api.revokeApiToken(ks.id, token.id)
      toast.success(t("api.revoked"))
      await load()
    } catch (error) {
      toast.error(t("api.revokeFailed", { error: (error as Error).message }))
    }
  }

  const reveal = async (token: ApiToken) => {
    setRevealingId(token.id)
    try {
      const result = await api.revealApiToken(ks.id, token.id)
      setRevealed({ row: token, token: result.token })
    } catch (error) {
      toast.error(t("api.revealFailed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setRevealingId(null)
    }
  }

  const copy = async (value: string, message: string) => {
    await navigator.clipboard.writeText(value)
    toast.success(message)
  }

  if (!canManage) {
    return <p className="text-sm text-muted-foreground">{t("api.onlyOwner")}</p>
  }

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="flex items-center gap-2 text-sm font-semibold"><KeyRound className="h-4 w-4" /> {t("api.title")}</h2>
        </div>
        <Button size="sm" onClick={openCreate}><Plus className="h-4 w-4" /> {t("api.createToken")}</Button>
      </div>

      <div className="rounded-lg border bg-muted/20 p-3">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs font-medium">{t("api.baseUrl")}</p>
            <code className="mt-1 block truncate text-xs text-muted-foreground">{baseUrl}</code>
          </div>
          <Button size="icon" variant="ghost" className="shrink-0" title={t("api.copyBaseUrl")} onClick={() => copy(baseUrl, t("api.baseUrlCopied"))}>
            <Copy className="h-4 w-4" />
          </Button>
        </div>
      </div>

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{t("common.name")}</TableHead><TableHead>{t("api.prefix")}</TableHead><TableHead>{t("api.scopes")}</TableHead>
              <TableHead>{t("common.status")}</TableHead><TableHead>{t("api.expires")}</TableHead><TableHead>{t("api.lastUsed")}</TableHead>
              <TableHead className="w-24" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={7} className="h-20 text-center text-muted-foreground">
                <Loader2 className="mr-2 inline h-4 w-4 animate-spin" /> {t("common.loading")}
              </TableCell></TableRow>
            ) : tokens.length === 0 ? (
              <TableRow><TableCell colSpan={7} className="h-20 text-center text-muted-foreground">
                {t("api.noTokens")}
              </TableCell></TableRow>
            ) : tokens.map((token) => (
              <TableRow key={token.id}>
                <TableCell className="font-medium">{token.name}</TableCell>
                <TableCell><code className="text-xs">{token.token_prefix}…</code></TableCell>
                <TableCell className="max-w-64">
                  <div className="flex flex-wrap gap-1">
                    {token.scopes.map((scope) => <Badge key={scope} variant="outline" className="text-[10px]">{scope}</Badge>)}
                  </div>
                </TableCell>
                <TableCell><StatusBadge status={token.status} /></TableCell>
                <TableCell className="whitespace-nowrap text-xs text-muted-foreground">{when(token.expires_at, locale, t("api.never"))}</TableCell>
                <TableCell className="whitespace-nowrap text-xs text-muted-foreground">{when(token.last_used_at, locale, t("api.never"))}</TableCell>
                <TableCell className="text-right">
                  <div className="flex justify-end gap-1">
                    {token.status === "active" && (
                      <span title={token.can_reveal ? t("api.revealToken") : t("api.revealUnavailable")}>
                        <Button size="icon" variant="ghost" className="h-8 w-8 text-muted-foreground"
                          aria-label={t("api.revealToken")} disabled={!token.can_reveal || revealingId === token.id}
                          onClick={() => reveal(token)}>
                          {revealingId === token.id
                            ? <Loader2 className="h-4 w-4 animate-spin" />
                            : <Eye className="h-4 w-4" />}
                        </Button>
                      </span>
                    )}
                    {token.status === "active" && (
                      <Button size="icon" variant="ghost" className="h-8 w-8 text-muted-foreground hover:text-destructive"
                        title={t("api.revokeToken")} onClick={() => revoke(token)}>
                        <Trash2 className="h-4 w-4" />
                      </Button>
                    )}
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("api.createToken")}</DialogTitle>
            <DialogDescription>{t("api.createDescription")}</DialogDescription>
          </DialogHeader>
          <div className="space-y-4 py-1">
            <div className="space-y-1.5">
              <Label htmlFor="token-name">{t("common.name")}</Label>
              <Input id="token-name" value={name} onChange={(event) => setName(event.target.value)}
                placeholder={t("api.namePlaceholder")} autoFocus />
            </div>
            <div className="space-y-2">
              <Label>{t("api.permissions")}</Label>
              {scopeOptions.map((scope) => (
                <label key={scope.value} className="flex cursor-pointer items-start gap-3 rounded-md border p-3">
                  <Checkbox checked={scopes.has(scope.value)} onCheckedChange={() => toggleScope(scope.value)} />
                  <span>
                    <span className="block text-sm font-medium">{scope.label} <code className="text-xs text-muted-foreground">{scope.value}</code></span>
                    <span className="block text-xs text-muted-foreground">{scope.description}</span>
                  </span>
                </label>
              ))}
            </div>
            <div className="space-y-1.5">
              <Label>{t("api.expires")}</Label>
              <Select value={expiry} onValueChange={setExpiry}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="30">{t("api.days", { count: 30 })}</SelectItem>
                  <SelectItem value="90">{t("api.days", { count: 90 })}</SelectItem>
                  <SelectItem value="365">{t("api.oneYear")}</SelectItem>
                  <SelectItem value="never">{t("api.never")}</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setCreateOpen(false)}>{t("common.cancel")}</Button>
            <Button onClick={create} disabled={creating || !name.trim() || scopes.size === 0}>
              {creating ? <Loader2 className="h-4 w-4 animate-spin" /> : <ShieldCheck className="h-4 w-4" />}
              {t("api.createToken")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={created !== null} onOpenChange={(open) => { if (!open) setCreated(null) }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("api.saveTitle")}</DialogTitle>
            <DialogDescription>{t("api.saveDescription")}</DialogDescription>
          </DialogHeader>
          {created && (
            <div className="space-y-4">
              <div className="rounded-md border bg-muted/40 p-3">
                <code className="break-all text-xs">{created.token}</code>
              </div>
              <Button className="w-full" variant="outline" onClick={() => copy(created.token, t("api.tokenCopied"))}>
                <Copy className="h-4 w-4" /> {t("api.copyToken")}
              </Button>
              <div className="rounded-md border p-3">
                <p className="mb-2 flex items-center gap-1.5 text-xs font-medium"><Check className="h-3.5 w-3.5" /> {t("api.example")}</p>
                <code className="block break-all text-xs text-muted-foreground">
                  curl -H &quot;Authorization: Bearer $ONTOPILOT_TOKEN&quot; &quot;{baseUrl}/ontology&quot;
                </code>
              </div>
            </div>
          )}
          <DialogFooter><Button onClick={() => setCreated(null)}>{t("common.done")}</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={revealed !== null} onOpenChange={(open) => { if (!open) setRevealed(null) }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("api.revealTitle")}</DialogTitle>
            <DialogDescription>{revealed ? t("api.revealDescription", { name: revealed.row.name }) : ""}</DialogDescription>
          </DialogHeader>
          {revealed && (
            <div className="space-y-4">
              <div className="rounded-md border bg-muted/40 p-3">
                <code className="break-all text-xs">{revealed.token}</code>
              </div>
              <Button className="w-full" variant="outline" onClick={() => copy(revealed.token, t("api.tokenCopied"))}>
                <Copy className="h-4 w-4" /> {t("api.copyToken")}
              </Button>
            </div>
          )}
          <DialogFooter><Button onClick={() => setRevealed(null)}>{t("common.done")}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
