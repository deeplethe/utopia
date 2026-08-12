import { useEffect, useState } from "react"
import { NavLink, useLocation } from "react-router-dom"
import {
  ChevronLeft, ChevronRight, Database, FileText, Inbox, KeyRound, LayoutDashboard,
  Network, PackageCheck, ScrollText, Tags, Users,
} from "lucide-react"
import { api } from "@/lib/api"
import type { KnowledgeSystem, ReviewCounts } from "@/lib/types"
import { useI18n, type MessageKey } from "@/lib/i18n"
import {
  Sidebar, SidebarContent, SidebarHeader, SidebarMenu, SidebarMenuButton, SidebarMenuItem,
  SidebarMenuSub, SidebarMenuSubButton, SidebarMenuSubItem,
} from "@/components/ui/sidebar"

const SECTIONS: { key: string; label: MessageKey; icon: typeof LayoutDashboard; ownerOnly?: boolean }[] = [
  { key: "overview", label: "sidebar.overview", icon: LayoutDashboard },
  { key: "ontology", label: "sidebar.ontology", icon: Network },
  { key: "vocabulary", label: "sidebar.vocabulary", icon: Tags },
  { key: "instances", label: "sidebar.instances", icon: Database },
  { key: "review", label: "sidebar.review", icon: Inbox },
  { key: "releases", label: "sidebar.releases", icon: PackageCheck },
  { key: "documents", label: "sidebar.documents", icon: FileText },
  { key: "history", label: "sidebar.history", icon: ScrollText },
  { key: "members", label: "sidebar.members", icon: Users },
  { key: "api", label: "sidebar.apiAccess", icon: KeyRound, ownerOnly: true },
]

// Review is a second-level menu; these are its sub-pages (/review/<key>). Each sub-page holds its
// own queue AND its learned-decision memory (resolution decisions under Entity resolution;
// reconciliation + duplicate-class decisions under Conflicts).
const REVIEW_SUBS: { key: string; label: MessageKey }[] = [
  { key: "conflicts", label: "sidebar.conflicts" },
  { key: "resolution", label: "sidebar.entityResolution" },
  { key: "terminology", label: "sidebar.terminology" },
  { key: "validation", label: "sidebar.validation" },
]

/**
 * Left sidebar for the current knowledge system: a flush panel with a right border. Its
 * header pins the KS name (so you always know which ontology you're editing while switching
 * sections) with a link back to the list; below are the section tabs (official
 * `SidebarMenuButton`, gray active state). Only rendered inside a KS. Needs a
 * `SidebarProvider` ancestor (App.tsx).
 */
function Count({ n, className = "" }: { n?: number; className?: string }) {
  if (!n) return null
  return (
    <span className={`rounded-full bg-primary/15 px-1.5 text-[10px] font-medium tabular-nums text-primary ${className}`}>
      {n}
    </span>
  )
}

export default function SideNav() {
  const { t } = useI18n()
  const pathname = useLocation().pathname
  const seg = pathname.split("/").filter(Boolean)
  const ksId = seg[0] === "knowledge" && seg[1] != null ? Number(seg[1]) : null
  const active = seg[2] ?? "overview"
  const activeSub = seg[3] ?? "conflicts"
  const [ks, setKs] = useState<KnowledgeSystem | null>(null)
  const [counts, setCounts] = useState<ReviewCounts | null>(null)
  // Review is a collapsible group; clicking it only expands/collapses (never navigates). Auto-open
  // when you're already on a review page so deep links reveal the sub-menu.
  const [reviewOpen, setReviewOpen] = useState(active === "review")
  useEffect(() => { if (active === "review") setReviewOpen(true) }, [active])

  useEffect(() => {
    if (ksId == null) return
    let cancelled = false
    api.getKS(ksId).then((k) => { if (!cancelled) setKs(k) }).catch(() => {})
    return () => { cancelled = true }
  }, [ksId])

  // Pending-item badges. Refetched on navigation (cheap), so resolving items then moving updates them.
  useEffect(() => {
    if (ksId == null) return
    let cancelled = false
    const loadCounts = () => {
      api.reviewCounts(ksId).then((next) => { if (!cancelled) setCounts(next) }).catch(() => {})
    }
    loadCounts()
    window.addEventListener("ontopilot:review-counts-changed", loadCounts)
    return () => {
      cancelled = true
      window.removeEventListener("ontopilot:review-counts-changed", loadCounts)
    }
  }, [ksId, pathname])

  if (ksId == null) return null

  return (
    <Sidebar
      collapsible="none"
      className="sticky top-14 hidden h-[calc(100svh-3.5rem)] w-60 shrink-0 self-start flex-col overflow-hidden border-r bg-background md:flex"
    >
      <SidebarHeader className="border-b py-3 pl-5 pr-3">
        <NavLink
          to="/"
          aria-label={t("sidebar.back")}
          className="group flex min-w-0 items-center gap-1.5"
        >
          <ChevronLeft className="h-4 w-4 shrink-0 text-muted-foreground transition-colors group-hover:text-foreground" />
          <span className="truncate text-sm font-semibold" title={ks?.name ?? undefined}>
            {ks?.name ?? "…"}
          </span>
        </NavLink>
      </SidebarHeader>

      <SidebarContent className="px-3 py-2">
        <SidebarMenu>
          {SECTIONS.filter((s) => !s.ownerOnly || ks?.my_role === "owner").map((s) =>
            s.key === "review" ? (
              <SidebarMenuItem key={s.key}>
                <SidebarMenuButton isActive={active === "review"} onClick={() => setReviewOpen((o) => !o)}>
                  <s.icon />
                  <span>{t(s.label)}</span>
                  <span className="ml-auto flex items-center gap-1.5">
                    {!reviewOpen && <Count n={counts?.total} />}
                    <ChevronRight className={`h-4 w-4 transition-transform ${reviewOpen ? "rotate-90" : ""}`} />
                  </span>
                </SidebarMenuButton>
                {reviewOpen && (
                  <SidebarMenuSub className="mr-0 gap-0 py-0 pr-0">
                    {REVIEW_SUBS.map((sub) => (
                      <SidebarMenuSubItem key={sub.key}>
                        <SidebarMenuSubButton asChild className="h-8 w-full data-active:font-medium" isActive={active === "review" && activeSub === sub.key}>
                          <NavLink to={`/knowledge/${ksId}/review/${sub.key}`}>
                            <span>{t(sub.label)}</span>
                            <Count n={counts?.[sub.key as keyof ReviewCounts]} className="ml-auto" />
                          </NavLink>
                        </SidebarMenuSubButton>
                      </SidebarMenuSubItem>
                    ))}
                  </SidebarMenuSub>
                )}
              </SidebarMenuItem>
            ) : (
              <SidebarMenuItem key={s.key}>
                <SidebarMenuButton asChild isActive={active === s.key}>
                  <NavLink to={`/knowledge/${ksId}/${s.key}`}>
                    <s.icon />
                    <span>{t(s.label)}</span>
                  </NavLink>
                </SidebarMenuButton>
              </SidebarMenuItem>
            ),
          )}
        </SidebarMenu>
      </SidebarContent>
    </Sidebar>
  )
}
