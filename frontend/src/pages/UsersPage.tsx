import { useCallback, useEffect, useMemo, useState } from "react"
import { toast } from "sonner"
import { KeyRound, Loader2, Plus, Search, ShieldCheck, Trash2, UserCog } from "lucide-react"
import { api } from "@/lib/api"
import { useAuth } from "@/lib/auth"
import { useI18n } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type { User } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"

export default function UsersPage() {
  const { user: me } = useAuth()
  const { t } = useI18n()
  const confirmAction = useConfirm()
  const [users, setUsers] = useState<User[]>([])
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState("")
  const [createOpen, setCreateOpen] = useState(false)
  const [uname, setUname] = useState("")
  const [pw, setPw] = useState("")
  const [isAdmin, setIsAdmin] = useState(false)
  const [creating, setCreating] = useState(false)
  const [pwUser, setPwUser] = useState<User | null>(null)
  const [newPw, setNewPw] = useState("")

  const filteredUsers = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase()
    if (!normalizedQuery) return users
    return users.filter((user) => user.username.toLocaleLowerCase().includes(normalizedQuery))
  }, [query, users])

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      setUsers(await api.listUsers())
    } catch (e) {
      toast.error(t("users.failedLoad", { error: (e as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [t])

  useEffect(() => { refresh() }, [refresh])

  const create = useCallback(async () => {
    if (!uname.trim() || !pw) return
    setCreating(true)
    try {
      await api.createUser(uname.trim(), pw, isAdmin)
      toast.success(t("users.created", { name: uname.trim() }))
      setCreateOpen(false); setUname(""); setPw(""); setIsAdmin(false)
      refresh()
    } catch (e) {
      toast.error(t("common.failedCreate", { error: (e as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setCreating(false)
    }
  }, [uname, pw, isAdmin, refresh, t])

  const patch = useCallback(async (u: User, p: { is_admin?: boolean; active?: boolean }) => {
    try {
      await api.updateUser(u.id, p)
      refresh()
    } catch (e) {
      toast.error(t("common.failedUpdate", { error: (e as Error).message.replace(/^\d+:\s*/, "") }))
    }
  }, [refresh, t])

  const resetPw = useCallback(async () => {
    if (!pwUser || !newPw) return
    try {
      await api.updateUser(pwUser.id, { password: newPw })
      toast.success(t("users.passwordReset", { name: pwUser.username }))
      setPwUser(null); setNewPw("")
    } catch (e) {
      toast.error(t("common.failedUpdate", { error: (e as Error).message.replace(/^\d+:\s*/, "") }))
    }
  }, [pwUser, newPw, t])

  const del = useCallback(async (u: User) => {
    if (!await confirmAction(t("users.deleteConfirm", { name: u.username }), { destructive: true })) return
    try {
      await api.deleteUser(u.id)
      toast.success(t("common.deleted"))
      refresh()
    } catch (e) {
      toast.error(t("common.failedDelete", { error: (e as Error).message.replace(/^\d+:\s*/, "") }))
    }
  }, [confirmAction, refresh, t])

  return (
    <div className="space-y-5">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">{t("users.title")}</h1>
        </div>
        <Button size="sm" onClick={() => setCreateOpen(true)}><Plus className="h-4 w-4" /> {t("users.new")}</Button>
      </div>

      <div className="rounded-lg border">
        <div className="flex flex-col gap-2 border-b px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="relative w-full sm:max-w-xs">
            <Search className="absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("users.searchPlaceholder")}
              aria-label={t("users.searchPlaceholder")}
              className="h-9 pl-8"
            />
          </div>
          {!loading && (
            <p className="text-xs text-muted-foreground">
              {t("users.resultCount", { shown: filteredUsers.length, total: users.length })}
            </p>
          )}
        </div>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{t("common.username")}</TableHead><TableHead>{t("common.role")}</TableHead><TableHead>{t("common.status")}</TableHead>
              <TableHead className="text-right">{t("common.actions")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={4} className="h-20 text-center text-muted-foreground">{t("common.loading")}</TableCell></TableRow>
            ) : filteredUsers.length === 0 ? (
              <TableRow><TableCell colSpan={4} className="h-24 text-center text-muted-foreground">{t("users.noMatches")}</TableCell></TableRow>
            ) : filteredUsers.map((u) => {
              const self = u.id === me?.id
              return (
                <TableRow key={u.id}>
                  <TableCell className="font-medium">
                    {u.username}
                    {self && <span className="ml-1.5 text-xs text-muted-foreground">{t("users.you")}</span>}
                  </TableCell>
                  <TableCell>
                    {u.is_admin ? <Badge className="gap-1"><ShieldCheck className="h-3 w-3" /> {t("common.admin")}</Badge> : <Badge variant="secondary">{t("common.user")}</Badge>}
                  </TableCell>
                  <TableCell>
                    {u.active ? <Badge variant="outline" className="border-emerald-500/40 text-emerald-600">{t("common.active")}</Badge> : <Badge variant="outline" className="text-muted-foreground">{t("common.disabled")}</Badge>}
                  </TableCell>
                  <TableCell className="space-x-1 text-right">
                    <Button size="sm" variant="ghost" title={t("users.toggleAdmin")} disabled={self} onClick={() => patch(u, { is_admin: !u.is_admin })}>
                      <UserCog className="h-3.5 w-3.5" /> {u.is_admin ? t("users.removeAdmin") : t("users.makeAdmin")}
                    </Button>
                    <Button size="sm" variant="ghost" title={t("users.toggleActive")} disabled={self} onClick={() => patch(u, { active: !u.active })}>
                      {u.active ? t("common.disable") : t("common.enable")}
                    </Button>
                    <Button size="icon" variant="ghost" className="h-8 w-8" title={t("users.resetPassword")} onClick={() => { setPwUser(u); setNewPw("") }}>
                      <KeyRound className="h-4 w-4" />
                    </Button>
                    <Button size="icon" variant="ghost" className="h-8 w-8 text-muted-foreground hover:text-destructive" title={t("common.delete")} disabled={self} onClick={() => del(u)}>
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      </div>

      {/* Create user */}
      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>{t("users.new")}</DialogTitle></DialogHeader>
          <div className="space-y-4 py-2">
            <div className="space-y-1.5">
              <Label htmlFor="new-uname">{t("common.username")}</Label>
              <Input id="new-uname" value={uname} onChange={(e) => setUname(e.target.value)} />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="new-pw">{t("users.initialPassword")}</Label>
              <Input id="new-pw" type="password" value={pw} onChange={(e) => setPw(e.target.value)} />
            </div>
            <label className="flex cursor-pointer items-center gap-2 text-sm">
              <Checkbox checked={isAdmin} onCheckedChange={(v) => setIsAdmin(!!v)} /> {t("users.makeAdminCheckbox")}
            </label>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setCreateOpen(false)}>{t("common.cancel")}</Button>
            <Button onClick={create} disabled={creating || !uname.trim() || !pw}>
              {creating && <Loader2 className="h-4 w-4 animate-spin" />} {t("common.create")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Reset password */}
      <Dialog open={!!pwUser} onOpenChange={(o) => !o && setPwUser(null)}>
        <DialogContent>
          <DialogHeader><DialogTitle>{t("users.resetPasswordFor", { name: pwUser?.username ?? "" })}</DialogTitle></DialogHeader>
          <div className="space-y-1.5 py-2">
            <Label htmlFor="reset-pw">{t("users.newPassword")}</Label>
            <Input id="reset-pw" type="password" value={newPw} onChange={(e) => setNewPw(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && resetPw()} />
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPwUser(null)}>{t("common.cancel")}</Button>
            <Button onClick={resetPw} disabled={!newPw}>{t("common.reset")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
