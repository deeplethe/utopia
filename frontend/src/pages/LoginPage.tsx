import { useState } from "react"
import { Loader2, Network } from "lucide-react"
import { useAuth } from "@/lib/auth"
import { useI18n } from "@/lib/i18n"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

export default function LoginPage() {
  const { login } = useAuth()
  const { t } = useI18n()
  const [username, setUsername] = useState("")
  const [password, setPassword] = useState("")
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!username.trim() || !password) return
    setBusy(true)
    setError(null)
    try {
      await login(username.trim(), password)
    } catch (err) {
      const msg = (err as Error).message.replace(/^\d+:\s*/, "")
      setError(msg || t("login.failed"))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-muted/30 px-4">
      <Card className="w-full max-w-sm">
        <CardHeader className="space-y-2 text-center">
          <div className="mx-auto flex h-10 w-10 items-center justify-center rounded-lg bg-primary text-primary-foreground">
            <Network className="h-5 w-5" />
          </div>
          <CardTitle className="text-lg">{t("login.title")}</CardTitle>
          <CardDescription>{t("login.description")}</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="username">{t("common.username")}</Label>
              <Input
                id="username" value={username} autoFocus autoComplete="username"
                onChange={(e) => setUsername(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="password">{t("login.password")}</Label>
              <Input
                id="password" type="password" value={password} autoComplete="current-password"
                onChange={(e) => setPassword(e.target.value)}
              />
            </div>
            {error && <p className="text-sm text-destructive">{error}</p>}
            <Button type="submit" className="w-full" disabled={busy || !username.trim() || !password}>
              {busy && <Loader2 className="h-4 w-4 animate-spin" />} {t("login.submit")}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}
