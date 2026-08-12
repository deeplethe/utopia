import { Check, Languages } from "lucide-react"
import { cn } from "@/lib/utils"
import { useI18n, type Locale } from "@/lib/i18n"

const OPTIONS: { key: Locale; nativeLabel: string; sample: string }[] = [
  { key: "en", nativeLabel: "English", sample: "Ontology governance workspace" },
  { key: "zh-CN", nativeLabel: "简体中文", sample: "本体治理工作台" },
]

export default function LanguageSection() {
  const { locale, setLocale, t } = useI18n()

  return (
    <div className="max-w-2xl space-y-6">
      <div>
        <h1 className="text-xl font-semibold tracking-tight">{t("language.title")}</h1>
        <p className="text-sm text-muted-foreground">{t("language.description")}</p>
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        {OPTIONS.map((option) => {
          const active = locale === option.key
          return (
            <button
              key={option.key}
              type="button"
              onClick={() => setLocale(option.key)}
              className={cn(
                "relative flex items-start gap-3 rounded-lg border p-4 text-left transition-colors",
                active
                  ? "border-primary bg-primary/5 text-foreground"
                  : "text-muted-foreground hover:border-foreground/30 hover:text-foreground",
              )}
            >
              {active && <Check className="absolute right-3 top-3 h-4 w-4 text-primary" />}
              <Languages className="mt-0.5 h-5 w-5 shrink-0" />
              <span>
                <span className="block text-sm font-medium text-foreground">{option.nativeLabel}</span>
                <span className="mt-1 block text-xs">{option.sample}</span>
              </span>
            </button>
          )
        })}
      </div>

      <p className="text-xs text-muted-foreground">{t("language.browserNote")}</p>
    </div>
  )
}
