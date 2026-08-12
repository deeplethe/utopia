import { useState } from "react"
import { toast } from "sonner"
import { Loader2 } from "lucide-react"
import { api } from "@/lib/api"
import { useAuth } from "@/lib/auth"
import { useI18n } from "@/lib/i18n"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

const err = (e: unknown) => (e as Error).message.replace(/^\d+:\s*/, "")

export default function ProfileSection() {
  const { user, refresh } = useAuth()
  const { t } = useI18n()
  const [name, setName] = useState(user?.display_name ?? "")
  const [savingName, setSavingName] = useState(false)

  const [cur, setCur] = useState("")
  const [np, setNp] = useState("")
  const [np2, setNp2] = useState("")
  const [savingPw, setSavingPw] = useState(false)

  const nameChanged = (name.trim() || null) !== (user?.display_name ?? null)

  const saveName = async () => {
    setSavingName(true)
    try {
      await api.updateMe({ display_name: name.trim() })
      await refresh()
      toast.success(name.trim() ? t("profile.nicknameUpdated") : t("profile.nicknameCleared"))
    } catch (e) {
      toast.error(err(e))
    } finally {
      setSavingName(false)
    }
  }

  const savePw = async () => {
    if (np !== np2) {
      toast.error(t("profile.passwordMismatch"))
      return
    }
    setSavingPw(true)
    try {
      await api.updateMe({ current_password: cur, new_password: np })
      toast.success(t("profile.passwordChanged"))
      setCur(""); setNp(""); setNp2("")
    } catch (e) {
      toast.error(err(e))
    } finally {
      setSavingPw(false)
    }
  }

  return (
    <div className="max-w-2xl space-y-6">
      <div>
        <h1 className="text-xl font-semibold tracking-tight">{t("profile.title")}</h1>
        <p className="text-sm text-muted-foreground">{t("profile.description")}</p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">{t("profile.displayName")}</CardTitle>
          <CardDescription>
            {t("profile.displayNameDescription", { username: user?.username ?? "" })}
            {user?.is_admin && <Badge variant="secondary" className="ml-2 text-[10px]">{t("common.admin")}</Badge>}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex items-end gap-2">
            <div className="flex-1 space-y-1.5">
              <Label htmlFor="p-name">{t("profile.nickname")}</Label>
              <Input id="p-name" value={name} onChange={(e) => setName(e.target.value)}
                placeholder={user?.username} maxLength={60}
                onKeyDown={(e) => e.key === "Enter" && nameChanged && saveName()} />
            </div>
            <Button onClick={saveName} disabled={savingName || !nameChanged}>
              {savingName && <Loader2 className="h-4 w-4 animate-spin" />} {t("common.save")}
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">{t("profile.changePassword")}</CardTitle>
          <CardDescription>{t("profile.passwordDescription")}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="space-y-1.5">
            <Label htmlFor="p-cur">{t("profile.currentPassword")}</Label>
            <Input id="p-cur" type="password" value={cur} onChange={(e) => setCur(e.target.value)} autoComplete="current-password" />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="p-new">{t("profile.newPassword")}</Label>
            <Input id="p-new" type="password" value={np} onChange={(e) => setNp(e.target.value)} autoComplete="new-password" />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="p-new2">{t("profile.confirmPassword")}</Label>
            <Input id="p-new2" type="password" value={np2} onChange={(e) => setNp2(e.target.value)} autoComplete="new-password"
              onKeyDown={(e) => e.key === "Enter" && cur && np && np2 && savePw()} />
          </div>
          <div className="pt-1">
            <Button onClick={savePw} disabled={savingPw || !cur || !np || !np2}>
              {savingPw && <Loader2 className="h-4 w-4 animate-spin" />} {t("profile.changePassword")}
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
