import { useEffect, useState } from "react"
import { useTheme } from "next-themes"
import { Check, Monitor, Moon, Sun } from "lucide-react"
import { cn } from "@/lib/utils"
import { useI18n, type MessageKey } from "@/lib/i18n"

const OPTIONS = [
  { key: "light", label: "appearance.light", icon: Sun },
  { key: "dark", label: "appearance.dark", icon: Moon },
  { key: "system", label: "appearance.system", icon: Monitor },
] as const

export default function AppearanceSection() {
  const { theme, setTheme } = useTheme()
  const { t } = useI18n()
  // next-themes resolves `theme` only after mount; gate the active highlight to avoid a flash
  // of the wrong option on first paint.
  const [mounted, setMounted] = useState(false)
  useEffect(() => setMounted(true), [])
  const active = mounted ? (theme ?? "system") : undefined

  return (
    <div className="max-w-2xl space-y-6">
      <div>
        <h1 className="text-xl font-semibold tracking-tight">{t("appearance.title")}</h1>
        <p className="text-sm text-muted-foreground">{t("appearance.description")}</p>
      </div>

      <div className="grid grid-cols-3 gap-3">
        {OPTIONS.map((o) => {
          const Icon = o.icon
          const isActive = active === o.key
          return (
            <button
              key={o.key}
              type="button"
              onClick={() => setTheme(o.key)}
              className={cn(
                "relative flex flex-col items-center gap-2 rounded-lg border p-5 text-sm transition-colors",
                isActive ? "border-primary bg-primary/5 text-foreground"
                         : "text-muted-foreground hover:border-foreground/30 hover:text-foreground",
              )}
            >
              {isActive && <Check className="absolute right-2 top-2 h-4 w-4 text-primary" />}
              <Icon className="h-6 w-6" />
              {t(o.label as MessageKey)}
            </button>
          )
        })}
      </div>
    </div>
  )
}
