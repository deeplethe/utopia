import { useEffect, useRef, useState, type ReactNode } from "react"
import { NavLink, Navigate, Route, Routes, useLocation } from "react-router-dom"
import { Bot, Loader2, LogOut } from "lucide-react"
import { MODE_DRAWS, resolvePreset } from "thinking-orbs"
import { cn } from "@/lib/utils"
import { useAuth } from "@/lib/auth"
import { useI18n } from "@/lib/i18n"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { SidebarProvider } from "@/components/ui/sidebar"
import SideNav from "@/components/SideNav"
import HomeAgentCopilot from "@/components/HomeAgentCopilot"
import LoginPage from "@/pages/LoginPage"
import KnowledgePage from "@/pages/KnowledgePage"
import OntologyPage from "@/pages/OntologyPage"
import DocumentDetailPage from "@/pages/DocumentDetailPage"
import DocumentationPage from "@/pages/ApiDocsPage"
import SettingsLayout from "@/pages/settings/SettingsLayout"

function TopLink({ to, active, children }: { to: string; active: boolean; children: ReactNode }) {
  return (
    <NavLink
      to={to}
      className={cn(
        "whitespace-nowrap rounded-md px-2 py-1.5 text-sm font-medium transition-colors sm:px-3",
        active ? "text-foreground" : "text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
    </NavLink>
  )
}

function PrimaryThinkingOrb() {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const size = 20
    const ratio = Math.min(2, window.devicePixelRatio || 1)
    const context = canvas.getContext("2d")
    if (!context) return
    const { mode, speed, opts } = resolvePreset("breathing", size)
    const draw = MODE_DRAWS[mode]
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches
    let frame = 0
    let primary = getComputedStyle(canvas).getPropertyValue("--primary").trim()

    canvas.width = Math.round(size * ratio)
    canvas.height = Math.round(size * ratio)

    const paint = (time: number) => {
      context.setTransform(ratio, 0, 0, ratio, 0, 0)
      context.clearRect(0, 0, size, size)
      draw(context, size, time, false, opts)
      context.save()
      context.globalCompositeOperation = "source-in"
      context.fillStyle = primary || "#1683a8"
      context.fillRect(0, 0, size, size)
      context.restore()
    }
    const tick = () => {
      paint(performance.now() / 1000 * speed)
      frame = window.requestAnimationFrame(tick)
    }
    const themeObserver = new MutationObserver(() => {
      primary = getComputedStyle(canvas).getPropertyValue("--primary").trim()
      if (reducedMotion) paint(0.6)
    })
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "data-theme", "style"],
    })

    if (reducedMotion) paint(0.6)
    else frame = window.requestAnimationFrame(tick)

    return () => {
      window.cancelAnimationFrame(frame)
      themeObserver.disconnect()
    }
  }, [])

  return (
    <canvas
      ref={canvasRef}
      aria-hidden="true"
      className="block h-5 w-5"
      style={{ width: 20, height: 20 }}
    />
  )
}

export default function App() {
  const { user, loading, logout } = useAuth()
  const { t } = useI18n()
  const location = useLocation()
  const seg = location.pathname.split("/").filter(Boolean)
  const [agentOpen, setAgentOpen] = useState(false)
  const [agentBusy, setAgentBusy] = useState(false)
  const hasHomeAgent = seg.length === 0
  const hasKnowledgeAgent = seg[0] === "knowledge"
    && Boolean(seg[1])
    && !(seg[2] === "documents" && seg.length > 3)
  const hasAgent = hasHomeAgent || hasKnowledgeAgent

  useEffect(() => {
    if (!hasAgent) {
      setAgentOpen(false)
      setAgentBusy(false)
    }
  }, [hasAgent])

  if (loading) {
    return (
      <div className="flex min-h-screen items-center justify-center">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    )
  }

  if (!user) return <LoginPage />

  const onKnowledge = seg.length === 0 || seg[0] === "knowledge"
  const onDocs = seg[0] === "docs" || seg[0] === "api-doc"
  const onSettings = seg[0] === "settings"

  const ontologyPage = (
    <OntologyPage
      key={seg[1]}
      agentOpen={agentOpen}
      onAgentOpenChange={setAgentOpen}
      onAgentBusyChange={setAgentBusy}
    />
  )

  return (
    <div className="flex min-h-svh flex-col bg-background text-foreground">
      {/* Fixed global nav — same across the whole app. */}
      <header className="sticky top-0 z-50 flex h-14 shrink-0 items-center gap-2 border-b bg-background/90 px-3 backdrop-blur sm:gap-4 md:px-6">
        <NavLink to="/" className="flex items-center">
          <span className="text-sm font-medium tracking-tight">OntoPilot</span>
        </NavLink>
        <nav className="flex min-w-0 flex-1 items-center gap-0 overflow-x-auto sm:gap-1">
          <TopLink to="/" active={onKnowledge}>{t("nav.knowledgeSystems")}</TopLink>
          <TopLink to="/docs" active={onDocs}>{t("nav.apiDocs")}</TopLink>
          <TopLink to="/settings" active={onSettings}>{t("nav.settings")}</TopLink>
        </nav>
        <div className="ml-auto flex shrink-0 items-center gap-2">
          {hasAgent && (
            <Button
              id="ontopilot-agent-trigger"
              type="button"
              variant="ghost"
              size="icon"
              aria-label={agentOpen
                ? "Close OntoPilot Agent"
                : agentBusy
                  ? "Open OntoPilot Agent — agent is working"
                  : "Open OntoPilot Agent"}
              aria-controls="ontopilot-agent-panel"
              aria-expanded={agentOpen}
              title="OntoPilot Agent"
              onClick={() => setAgentOpen((current) => !current)}
              className={cn(
                "transition-colors duration-200",
                agentOpen && "relative z-[61] bg-primary/20 text-primary hover:bg-primary/25 hover:text-primary",
                !agentOpen && agentBusy && "text-primary hover:text-primary",
              )}
            >
              {!agentOpen && agentBusy ? <PrimaryThinkingOrb /> : <Bot className="h-4 w-4" />}
            </Button>
          )}
          <Button asChild variant="ghost" size="icon" className="hidden lg:inline-flex">
            <a
              href="https://github.com/deeplethe/ontopilot"
              target="_blank"
              rel="noopener noreferrer"
              aria-label="GitHub"
              title="GitHub"
            >
              <svg viewBox="0 0 24 24" className="h-4 w-4 fill-current" aria-hidden="true">
                <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.084-.729.084-.729 1.205.084 1.84 1.237 1.84 1.237 1.07 1.835 2.809 1.305 3.495.998.108-.776.418-1.305.762-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
              </svg>
            </a>
          </Button>
          <span className="hidden items-center gap-1.5 text-sm text-muted-foreground lg:flex">
            {user.display_name || user.username}
            {user.is_admin && <Badge variant="secondary" className="text-[10px]">{t("common.admin")}</Badge>}
          </span>
          <Button variant="ghost" size="sm" onClick={() => logout()} title={t("nav.logout")}>
            <LogOut className="h-4 w-4" /> <span className="hidden sm:inline">{t("nav.logout")}</span>
          </Button>
        </div>
      </header>

      {/* Body: flush left sidebar (inside a KS) + content. SidebarProvider supplies the
          context the shadcn Sidebar primitives in <SideNav /> require. */}
      <SidebarProvider className="min-h-0 flex-1 items-start">
        <SideNav />
        <main className="min-w-0 flex-1 p-4 md:p-6">
          <Routes>
            <Route path="/" element={<KnowledgePage />} />
            <Route path="/docs" element={<DocumentationPage />} />
            <Route path="/docs/:docId" element={<DocumentationPage />} />
            <Route path="/api-doc" element={<Navigate to="/docs" replace />} />
            <Route path="/knowledge/:id" element={ontologyPage} />
            <Route path="/knowledge/:id/documents/:documentId" element={<DocumentDetailPage />} />
            <Route path="/knowledge/:id/:section" element={ontologyPage} />
            <Route path="/knowledge/:id/:section/:sub" element={ontologyPage} />
            {/* Users + model config now live under Settings; keep the old /users path working. */}
            <Route path="/users" element={<Navigate to="/settings/users" replace />} />
            <Route path="/settings" element={<Navigate to="/settings/profile" replace />} />
            <Route path="/settings/:section" element={<SettingsLayout />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </main>
      </SidebarProvider>
      {hasHomeAgent && (
        <HomeAgentCopilot
          open={agentOpen}
          onOpenChange={setAgentOpen}
          onBusyChange={setAgentBusy}
        />
      )}
    </div>
  )
}
