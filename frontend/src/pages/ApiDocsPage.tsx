import { useEffect, useState } from "react"
import { useNavigate, useParams } from "react-router-dom"
import { cn } from "@/lib/utils"
import { useI18n } from "@/lib/i18n"
import MarkdownDocument from "@/components/MarkdownDocument"

type DocItem = { id: string; label: string }
type DocGroup = { label: string; items: DocItem[] }

const ZH_NAV: DocGroup[] = [
  { label: "了解 OntoPilot", items: [
    { id: "overview", label: "产品概览" },
    { id: "concepts", label: "核心概念" },
    { id: "workflow", label: "推荐工作流" },
  ] },
  { label: "产品功能", items: [
    { id: "capabilities", label: "功能地图" },
    { id: "documents", label: "文档与抽取" },
    { id: "ontology", label: "本体与术语" },
    { id: "instances", label: "实例与溯源" },
    { id: "review", label: "审阅与质量门禁" },
    { id: "release", label: "版本发布" },
  ] },
  { label: "系统设计", items: [
    { id: "architecture", label: "系统架构" },
    { id: "storage", label: "数据与存储边界" },
    { id: "security", label: "权限、安全与审计" },
  ] },
  { label: "集成开发", items: [
    { id: "api-auth", label: "API 认证与地址" },
    { id: "api-methods", label: "接口与调用示例" },
    { id: "mcp", label: "MCP 与 Agent 集成" },
  ] },
  { label: "部署运维", items: [
    { id: "deployment", label: "Docker 与配置" },
  ] },
]

const EN_NAV: DocGroup[] = [
  { label: "Understand OntoPilot", items: [
    { id: "overview", label: "Product overview" },
    { id: "concepts", label: "Core concepts" },
    { id: "workflow", label: "Recommended workflow" },
  ] },
  { label: "Product features", items: [
    { id: "capabilities", label: "Capability map" },
    { id: "documents", label: "Documents and extraction" },
    { id: "ontology", label: "Ontology and vocabulary" },
    { id: "instances", label: "Instances and provenance" },
    { id: "review", label: "Review and quality gates" },
    { id: "release", label: "Release lifecycle" },
  ] },
  { label: "System design", items: [
    { id: "architecture", label: "Architecture" },
    { id: "storage", label: "Data and storage boundaries" },
    { id: "security", label: "Permissions, security, and audit" },
  ] },
  { label: "Integration", items: [
    { id: "api-auth", label: "API authentication and URLs" },
    { id: "api-methods", label: "Endpoints and examples" },
    { id: "mcp", label: "MCP and agent integration" },
  ] },
  { label: "Operations", items: [
    { id: "deployment", label: "Docker and configuration" },
  ] },
]

const DOC_MODULES = import.meta.glob("../content/docs/**/*.md", {
  query: "?raw",
  import: "default",
}) as Record<string, () => Promise<string>>

const DOC_IDS = new Set(ZH_NAV.flatMap((group) => group.items.map((item) => item.id)))

export default function DocumentationPage() {
  const { locale } = useI18n()
  const navigate = useNavigate()
  const { docId } = useParams<{ docId?: string }>()
  const language = locale.toLowerCase().startsWith("zh") ? "zh-CN" : "en"
  const groups = language === "zh-CN" ? ZH_NAV : EN_NAV
  const activeId = docId && DOC_IDS.has(docId) ? docId : "overview"
  const [source, setSource] = useState("")

  useEffect(() => {
    window.scrollTo({ top: 0, left: 0, behavior: "auto" })
    setSource("")
    let cancelled = false
    const loader = DOC_MODULES[`../content/docs/${language}/${activeId}.md`]
      ?? DOC_MODULES[`../content/docs/zh-CN/${activeId}.md`]
    void loader?.().then((markdown) => {
      if (!cancelled) setSource(markdown)
    })
    return () => { cancelled = true }
  }, [activeId, language])

  return (
    <div className="w-full">
      <div className="grid items-start gap-8 lg:grid-cols-[15rem_minmax(0,78rem)] xl:grid-cols-[17rem_minmax(0,78rem)]">
        <aside className="border-b pb-5 lg:sticky lg:top-20 lg:max-h-[calc(100svh-6rem)] lg:overflow-y-auto lg:border-b-0 lg:border-r lg:pb-0 lg:pr-5">
          <nav aria-label={language === "zh-CN" ? "文档目录" : "Documentation navigation"} className="space-y-4">
            {groups.map((group) => (
              <div key={group.label}>
                <p className="mb-1.5 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">{group.label}</p>
                <div className="relative border-l">
                  {group.items.map((item) => (
                    <button
                      key={item.id}
                      type="button"
                      onClick={() => navigate(`/docs/${item.id}`)}
                      className={cn(
                        "relative -ml-px flex w-[calc(100%+1px)] items-center border-l-2 px-3 py-1.5 text-left text-xs transition-colors",
                        activeId === item.id
                          ? "border-primary font-medium text-primary"
                          : "border-transparent text-muted-foreground hover:text-foreground",
                      )}
                    >
                      {item.label}
                    </button>
                  ))}
                </div>
              </div>
            ))}
          </nav>
        </aside>

        <article className="min-w-0 w-full max-w-[78rem]">
          {source
            ? <MarkdownDocument source={source} documentId={`${language}/${activeId}`} />
            : <div className="h-48 animate-pulse rounded-xl bg-muted/60" />}
        </article>
      </div>
    </div>
  )
}
