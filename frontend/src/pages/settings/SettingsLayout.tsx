import { NavLink, Navigate, useParams } from "react-router-dom"
import type { ComponentType } from "react"
import { Cpu, Languages, Palette, User as UserIcon, Users as UsersIcon } from "lucide-react"
import { cn } from "@/lib/utils"
import { useAuth } from "@/lib/auth"
import { useI18n, type MessageKey } from "@/lib/i18n"
import UsersPage from "@/pages/UsersPage"
import ModelEndpointsPage from "@/pages/SettingsPage"
import ProfileSection from "./ProfileSection"
import AppearanceSection from "./AppearanceSection"
import LanguageSection from "./LanguageSection"

interface Item {
  key: string
  label: MessageKey
  icon: ComponentType<{ className?: string }>
  element: React.ReactNode
}
interface Group {
  title: MessageKey
  admin?: boolean
  items: Item[]
}

// Elements are created once at module load, so switching sections doesn't remount siblings and
// re-navigating back to a section keeps its own (fresh) mount.
const GROUPS: Group[] = [
  {
    title: "settings.group.account",
    items: [
      { key: "profile", label: "settings.profile", icon: UserIcon, element: <ProfileSection /> },
      { key: "appearance", label: "settings.appearance", icon: Palette, element: <AppearanceSection /> },
      { key: "language", label: "settings.language", icon: Languages, element: <LanguageSection /> },
    ],
  },
  {
    title: "settings.group.admin",
    admin: true,
    items: [
      { key: "users", label: "settings.users", icon: UsersIcon, element: <UsersPage /> },
      { key: "models", label: "settings.models", icon: Cpu, element: <ModelEndpointsPage /> },
    ],
  },
]

export default function SettingsLayout() {
  const { section } = useParams()
  const { user } = useAuth()
  const { t } = useI18n()
  const isAdmin = !!user?.is_admin

  const current = GROUPS.flatMap((g) => g.items.map((it) => ({ ...it, admin: g.admin }))).find(
    (it) => it.key === section,
  )
  // Unknown section, or an admin-only section for a non-admin → bounce to Profile.
  if (!current || (current.admin && !isAdmin)) return <Navigate to="/settings/profile" replace />

  return (
    <div className="flex flex-col gap-8 md:flex-row">
      <aside className="shrink-0 md:w-52">
        <h1 className="mb-3 px-2 text-lg font-semibold tracking-tight">{t("settings.title")}</h1>
        <nav className="space-y-4">
          {GROUPS.filter((g) => !g.admin || isAdmin).map((g) => (
            <div key={g.title}>
              <div className="mb-1 px-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                {t(g.title)}
              </div>
              <div className="space-y-0.5">
                {g.items.map((it) => {
                  const Icon = it.icon
                  return (
                    <NavLink
                      key={it.key}
                      to={`/settings/${it.key}`}
                      className={({ isActive }) =>
                        cn(
                          "flex items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors",
                          isActive
                            ? "bg-muted font-medium text-foreground"
                            : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
                        )
                      }
                    >
                      <Icon className="h-4 w-4" /> {t(it.label)}
                    </NavLink>
                  )
                })}
              </div>
            </div>
          ))}
        </nav>
      </aside>

      <div className="min-w-0 flex-1">{current.element}</div>
    </div>
  )
}
