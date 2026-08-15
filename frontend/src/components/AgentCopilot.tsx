import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react"
import DOMPurify from "dompurify"
import {
  AlertCircle,
  Bot,
  Check,
  ChevronDown,
  Loader2,
  MessageSquare,
  Pencil,
  Plus,
  Search,
  Send,
  Sparkles,
  Trash2,
  Wrench,
  X,
} from "lucide-react"
import { Marked } from "marked"
import { createPortal } from "react-dom"
import { toast } from "sonner"
import { ThinkingOrb } from "thinking-orbs"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import type {
  AgentConversation,
  AgentConversationTurn,
  AgentProposal,
  AgentStreamEvent,
  AgentTraceStep,
  KnowledgeSystem,
} from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"

type AgentActivity =
  | {
      id: number
      kind: "progress"
      phase?: string
      title: string
      detail?: string
    }
  | {
      id: number
      kind: "commentary"
      text: string
    }
  | {
      id: number
      kind: "trace"
      trace: AgentTraceStep
    }

type DisplayAgentActivity =
  | Extract<AgentActivity, { kind: "progress" }>
  | Extract<AgentActivity, { kind: "commentary" }>
  | {
      id: number
      kind: "trace-group"
      traces: AgentTraceStep[]
    }

type ChatMessage = {
  id: number
  role: "user" | "assistant"
  content: string
  trace?: AgentTraceStep[]
  activity?: AgentActivity[]
  proposal?: AgentProposal | null
  previewRequested?: boolean
  streaming?: boolean
  error?: string
}

type AgentCopilotProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  onBusyChange?: (busy: boolean) => void
  ksId: number | null
  canWrite: boolean
  onPreviewProposal: (proposal: AgentProposal) => void
  knowledgeSystems?: KnowledgeSystem[]
  knowledgeSystemsLoading?: boolean
  onKnowledgeSystemChange?: (ksId: number) => void
}

const TOOL_LABELS: Record<string, { en: string; zh: string }> = {
  get_workspace_context: { en: "Workspace status", zh: "工作区状态" },
  get_ontology: { en: "Full ontology", zh: "完整本体" },
  get_ontology_neighborhood: { en: "Entity neighborhood", zh: "实体邻域" },
  search_ontology: { en: "Ontology search", zh: "本体搜索" },
  list_documents: { en: "Source documents", zh: "来源文档" },
  list_vocabulary_concepts: { en: "Vocabulary", zh: "领域词汇" },
  resolve_term: { en: "Term resolution", zh: "术语解析" },
  list_individuals: { en: "Instances", zh: "实例列表" },
  get_individual: { en: "Instance details", zh: "实例详情" },
  query_knowledge: { en: "Knowledge query", zh: "知识查询" },
  list_review_items: { en: "Review queues", zh: "审核队列" },
  get_conflict_context: { en: "Conflict evidence", zh: "冲突证据" },
  get_conflicts_context: { en: "Conflict evidence batch", zh: "批量冲突证据" },
  get_history: { en: "Change history", zh: "变更历史" },
  list_releases: { en: "Releases", zh: "发布版本" },
  preview_ontology_changes: { en: "Semantic preview", zh: "语义预检" },
}

function readableToolName(tool: string, zh: boolean) {
  const label = TOOL_LABELS[tool]
  return label ? (zh ? label.zh : label.en) : tool.replaceAll("_", " ")
}

function runningLabel(message: ChatMessage, zh: boolean) {
  const current = message.activity?.at(-1)
  if (current?.kind === "progress") return current.title
  if (current?.kind === "trace") {
    const tool = readableToolName(current.trace.tool, zh)
    return zh ? `正在分析${tool}结果` : `Reviewing ${tool} results`
  }
  if (current?.kind === "commentary") {
    const text = current.text.toLocaleLowerCase()
    if (/冲突|conflict/.test(text)) return zh ? "正在核对冲突证据" : "Checking conflict evidence"
    if (/术语|词汇|terminology|vocabulary/.test(text)) return zh ? "正在核对术语状态" : "Checking terminology"
    if (/审核|审批|review|approval/.test(text)) return zh ? "正在读取审核项目" : "Reading review items"
    if (/实体|entity|instance/.test(text)) return zh ? "正在核对相关实体" : "Checking related entities"
    if (/本体|结构|ontology|schema/.test(text)) return zh ? "正在检查本体结构" : "Inspecting ontology structure"
    if (/文档|来源|document|source/.test(text)) return zh ? "正在核对来源文档" : "Checking source documents"
    if (/发布|版本|release|version/.test(text)) return zh ? "正在检查发布记录" : "Checking release records"
    return zh ? "正在继续分析" : "Continuing the analysis"
  }
  return zh ? "正在理解你的问题" : "Understanding your request"
}

function RunningStatus({
  label,
  framed = false,
}: {
  label: string
  framed?: boolean
}) {
  return (
    <div
      role="status"
      aria-label={label}
      className={`flex min-w-0 items-center gap-2 text-xs text-muted-foreground ${framed
        ? "rounded-md bg-muted/35 px-2.5 py-2"
        : "h-7 px-1"
      }`}
    >
      <ThinkingOrb
        state="breathing"
        size={20}
        speed={0.9}
        className="shrink-0 opacity-80"
        aria-hidden="true"
      />
      <span className="agent-status-sweep min-w-0 flex-1 truncate">{label}</span>
    </div>
  )
}

function compactToolArguments(args: Record<string, unknown>) {
  const text = Object.keys(args).length ? JSON.stringify(args) : "{}"
  return text.length > 180 ? `${text.slice(0, 177)}…` : text
}

function trimRunningActivity(activity: AgentActivity[] | undefined) {
  if (!activity?.length) return activity
  const last = activity.at(-1)
  return last?.kind === "progress" ? activity.slice(0, -1) : activity
}

function groupAdjacentToolActivity(activity: AgentActivity[]): DisplayAgentActivity[] {
  const grouped: DisplayAgentActivity[] = []
  for (const item of activity) {
    if (item.kind !== "trace") {
      grouped.push(item)
      continue
    }
    const previous = grouped.at(-1)
    if (previous?.kind === "trace-group" && previous.traces.at(-1)?.tool === item.trace.tool) {
      previous.traces.push(item.trace)
      continue
    }
    grouped.push({ id: item.id, kind: "trace-group", traces: [item.trace] })
  }
  return grouped
}

function groupedTraceSummary(traces: AgentTraceStep[], zh: boolean) {
  const summaries = [...new Set(traces.map((trace) => trace.summary.trim()).filter(Boolean))]
  const summary = summaries.join(" · ")
  if (traces.length === 1) return summary
  const count = zh ? `${traces.length} 次调用` : `${traces.length} calls`
  return summary ? `${count} · ${summary}` : count
}

const CONVERSATION_STORAGE_PREFIX = "ontopilot:agent:conversation:"

function conversationStorageKey(ksId: number) {
  return `${CONVERSATION_STORAGE_PREFIX}${ksId}`
}

function savedConversationId(ksId: number) {
  try {
    const value = Number(localStorage.getItem(conversationStorageKey(ksId)))
    return Number.isInteger(value) && value > 0 ? value : null
  } catch {
    return null
  }
}

function rememberConversationId(ksId: number, conversationId: number | null) {
  try {
    const key = conversationStorageKey(ksId)
    if (conversationId == null) localStorage.removeItem(key)
    else localStorage.setItem(key, String(conversationId))
  } catch {
    // Conversation persistence is a convenience; private browsing may block localStorage.
  }
}

function normalizeTraceStep(value: unknown): AgentTraceStep | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null
  const candidate = value as Record<string, unknown>
  if (typeof candidate.tool !== "string") return null
  return {
    tool: candidate.tool,
    arguments: candidate.arguments && typeof candidate.arguments === "object" && !Array.isArray(candidate.arguments)
      ? candidate.arguments as Record<string, unknown>
      : {},
    summary: typeof candidate.summary === "string" ? candidate.summary : "MCP tool completed",
    reason: typeof candidate.reason === "string" ? candidate.reason : undefined,
  }
}

function turnTrace(turn: AgentConversationTurn) {
  const direct = turn.trace?.length ? turn.trace : turn.tool_trace
  if (direct?.length) return direct
  return (turn.events ?? []).flatMap((event) => {
    const data = event.data
    const candidate = event.trace ?? data?.trace ?? data
    const trace = normalizeTraceStep(candidate)
    return trace ? [trace] : []
  })
}

function turnActivity(
  turn: AgentConversationTurn,
  trace: AgentTraceStep[],
  nextActivityId: { current: number },
) {
  const activity: AgentActivity[] = []
  const events = [...(turn.events ?? [])].sort((left, right) => (left.idx ?? 0) - (right.idx ?? 0))
  for (const event of events) {
    const kind = event.kind ?? event.type
    if (kind === "commentary") {
      const text = event.data?.text
      if (typeof text === "string" && text.trim()) {
        activity.push({ id: nextActivityId.current++, kind: "commentary", text: text.trim() })
      }
      continue
    }
    if (kind !== "trace") continue
    const candidate = event.trace ?? event.data?.trace ?? event.data
    const step = normalizeTraceStep(candidate)
    if (step) activity.push({ id: nextActivityId.current++, kind: "trace", trace: step })
  }
  if (activity.some((item) => item.kind === "trace") || !trace.length) return activity
  return [
    ...activity,
    ...trace.map((step) => ({
      id: nextActivityId.current++,
      kind: "trace" as const,
      trace: step,
    })),
  ]
}

function turnContent(turn: AgentConversationTurn) {
  let replayed = ""
  let hasTranscriptEvent = false
  const events = [...(turn.events ?? [])].sort((left, right) => (left.idx ?? 0) - (right.idx ?? 0))
  for (const event of events) {
    const kind = event.kind ?? event.type
    if (kind === "answer_reset") {
      replayed = ""
      hasTranscriptEvent = true
      continue
    }
    if (kind !== "token" && kind !== "delta") continue
    const delta = event.data?.delta
    if (typeof delta !== "string") continue
    replayed += delta
    hasTranscriptEvent = true
  }
  // Event replay preserves answer_reset boundaries for running/failed turns and makes a
  // refreshed transcript match what was streamed. Older rows without events fall back to
  // the finalized turn content.
  return hasTranscriptEvent ? replayed : (turn.content ?? "")
}

function normalizeProposal(value: unknown): AgentProposal | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null
  const candidate = value as Record<string, unknown>
  return Array.isArray(candidate.operations) ? candidate as unknown as AgentProposal : null
}

function turnProposal(turn: AgentConversationTurn) {
  if (turn.proposal) return turn.proposal
  let proposal: AgentProposal | null = null
  const events = [...(turn.events ?? [])].sort((left, right) => (left.idx ?? 0) - (right.idx ?? 0))
  for (const event of events) {
    if ((event.kind ?? event.type) !== "proposal") continue
    const data = event.data
    proposal = normalizeProposal(data?.proposal ?? data)
  }
  return proposal
}

function displayConversationTitle(conversation: AgentConversation | undefined, zh: boolean) {
  const title = conversation?.title?.trim() || conversation?.first_user_message?.trim()
  return title || (zh ? "新对话" : "New conversation")
}

function restoreChatMessages(
  turns: AgentConversationTurn[],
  nextMessageId: { current: number },
  nextActivityId: { current: number },
) {
  const restored = turns
    .filter((turn) => turn.role === "user" || turn.role === "assistant")
    .map((turn): ChatMessage => {
      const trace = turn.role === "assistant" ? turnTrace(turn) : []
      return {
        id: turn.id,
        role: turn.role,
        content: turnContent(turn),
        trace,
        activity: turn.role === "assistant" ? turnActivity(turn, trace, nextActivityId) : [],
        proposal: turn.role === "assistant" ? turnProposal(turn) : null,
        streaming: false,
        error: turn.status === "failed" || turn.status === "cancelled"
          ? (turn.error || turn.fail_reason || "Previous run did not complete")
          : undefined,
      }
    })
  const largestId = restored.reduce((largest, message) => Math.max(largest, message.id), 0)
  nextMessageId.current = Math.max(nextMessageId.current, largestId + 1)
  return restored
}

const agentMarkdown = new Marked({ gfm: true, breaks: true })

function AgentMarkdown({ source }: { source: string }) {
  const html = useMemo(() => DOMPurify.sanitize(agentMarkdown.parse(source) as string, {
    ALLOWED_TAGS: [
      "a", "blockquote", "br", "code", "del", "em", "h1", "h2", "h3", "h4", "hr",
      "li", "ol", "p", "pre", "strong", "table", "tbody", "td", "th", "thead", "tr", "ul",
    ],
    ALLOWED_ATTR: ["href", "title"],
    FORBID_ATTR: ["class", "id", "style"],
  }), [source])

  return <div className="agent-markdown" dangerouslySetInnerHTML={{ __html: html }} />
}

export default function AgentCopilot({
  open,
  onOpenChange,
  onBusyChange,
  ksId,
  canWrite,
  onPreviewProposal,
  knowledgeSystems,
  knowledgeSystemsLoading = false,
  onKnowledgeSystemChange,
}: AgentCopilotProps) {
  const { locale } = useI18n()
  const zh = locale === "zh-CN"
  const [input, setInput] = useState("")
  const [busy, setBusy] = useState(false)
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [panelPresent, setPanelPresent] = useState(open)
  const [panelVisible, setPanelVisible] = useState(false)
  const [conversationId, setConversationId] = useState<number | null>(null)
  const [conversations, setConversations] = useState<AgentConversation[]>([])
  const [conversationsOpen, setConversationsOpen] = useState(false)
  const [conversationsLoading, setConversationsLoading] = useState(false)
  const [historyLoading, setHistoryLoading] = useState(false)
  const [conversationError, setConversationError] = useState("")
  const [renamingConversationId, setRenamingConversationId] = useState<number | null>(null)
  const [renameDraft, setRenameDraft] = useState("")
  const [renameSaving, setRenameSaving] = useState(false)
  const [deleteConfirmConversationId, setDeleteConfirmConversationId] = useState<number | null>(null)
  const [deletingConversationId, setDeletingConversationId] = useState<number | null>(null)
  const [knowledgePickerOpen, setKnowledgePickerOpen] = useState(false)
  const [knowledgeSearch, setKnowledgeSearch] = useState("")
  const [panelAnchor, setPanelAnchor] = useState({
    left: 16,
    right: 16,
    top: 64,
    arrowLeft: 272,
  })
  const nextId = useRef(1)
  const nextActivityId = useRef(1)
  const activeRequest = useRef<AbortController | null>(null)
  const activeKsIdRef = useRef(ksId)
  const conversationRef = useRef<HTMLDivElement>(null)
  const conversationsMenuRef = useRef<HTMLDivElement>(null)
  const historyRequestVersion = useRef(0)
  const conversationListRequestVersion = useRef(0)
  const renameActionRef = useRef<"idle" | "saving" | "cancelled">("idle")
  activeKsIdRef.current = ksId

  const selectedKnowledgeSystem = useMemo(
    () => knowledgeSystems?.find((system) => system.id === ksId) ?? null,
    [knowledgeSystems, ksId],
  )
  const visibleKnowledgeSystems = useMemo(() => {
    const term = knowledgeSearch.trim().toLocaleLowerCase()
    if (!term) return knowledgeSystems ?? []
    return (knowledgeSystems ?? []).filter((system) => (
      system.name.toLocaleLowerCase().includes(term)
      || system.description?.toLocaleLowerCase().includes(term)
    ))
  }, [knowledgeSearch, knowledgeSystems])

  useLayoutEffect(() => {
    if (!open || !panelPresent || historyLoading) return
    const conversation = conversationRef.current
    if (conversation) conversation.scrollTop = conversation.scrollHeight
  }, [conversationId, historyLoading, messages, open, panelPresent])

  useEffect(() => {
    if (open) {
      setPanelPresent(true)
      const frame = window.requestAnimationFrame(() => setPanelVisible(true))
      return () => window.cancelAnimationFrame(frame)
    }
    setPanelVisible(false)
    if (!panelPresent) return
    const timer = window.setTimeout(() => setPanelPresent(false), 150)
    return () => window.clearTimeout(timer)
  }, [open, panelPresent])

  useEffect(() => {
    onBusyChange?.(busy)
    return () => {
      if (busy) onBusyChange?.(false)
    }
  }, [busy, onBusyChange])

  useEffect(() => () => {
    activeRequest.current?.abort()
    activeRequest.current = null
  }, [])

  useLayoutEffect(() => {
    if (!open) return
    const alignToTrigger = () => {
      const trigger = document.getElementById("ontopilot-agent-trigger")
      if (!trigger) return
      const triggerRect = trigger.getBoundingClientRect()
      const headerRect = trigger.closest("header")?.getBoundingClientRect()
      const viewportWidth = document.documentElement.clientWidth || window.innerWidth
      const gutter = viewportWidth >= 768 ? 16 : 12
      const panelRight = gutter
      const preferredWidth = 608
      const panelWidth = Math.min(preferredWidth, viewportWidth - gutter - panelRight)
      const panelLeft = viewportWidth - panelRight - panelWidth
      const triggerCenter = triggerRect.left + triggerRect.width / 2
      const panelTop = Math.max(triggerRect.bottom, headerRect?.bottom ?? triggerRect.bottom) + 8
      setPanelAnchor({
        left: panelLeft,
        right: panelRight,
        top: panelTop,
        arrowLeft: Math.max(20, Math.min(panelWidth - 20, triggerCenter - panelLeft)),
      })
    }
    alignToTrigger()
    const trigger = document.getElementById("ontopilot-agent-trigger")
    const header = trigger?.closest("header")
    const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(alignToTrigger)
    if (trigger) resizeObserver?.observe(trigger)
    if (header) resizeObserver?.observe(header)
    window.addEventListener("resize", alignToTrigger)
    window.addEventListener("scroll", alignToTrigger, true)
    return () => {
      resizeObserver?.disconnect()
      window.removeEventListener("resize", alignToTrigger)
      window.removeEventListener("scroll", alignToTrigger, true)
    }
  }, [open])

  const stopActiveRequest = () => {
    const request = activeRequest.current
    activeRequest.current = null
    request?.abort()
    setBusy(false)
  }

  const clearConversationView = () => {
    stopActiveRequest()
    historyRequestVersion.current += 1
    setHistoryLoading(false)
    setMessages([])
    setInput("")
  }

  const startNewConversation = () => {
    if (busy) return
    clearConversationView()
    setConversationId(null)
    setConversationsOpen(false)
    setRenamingConversationId(null)
    setDeleteConfirmConversationId(null)
    setConversationError("")
    if (ksId != null) rememberConversationId(ksId, null)
  }

  const loadConversation = async (requestKsId: number, nextConversationId: number, notify = true) => {
    const requestVersion = ++historyRequestVersion.current
    setHistoryLoading(true)
    setConversationError("")
    try {
      const detail = await api.getAgentConversation(requestKsId, nextConversationId)
      if (
        requestVersion !== historyRequestVersion.current
        || requestKsId !== activeKsIdRef.current
      ) return false
      setMessages(restoreChatMessages(detail.turns ?? [], nextId, nextActivityId))
      setConversations((current) => {
        const summary: AgentConversation = {
          id: detail.id,
          knowledge_system_id: detail.knowledge_system_id,
          title: detail.title,
          first_user_message: detail.first_user_message,
          turn_count: detail.turn_count,
          created_at: detail.created_at,
          updated_at: detail.updated_at,
        }
        const existing = current.some((conversation) => conversation.id === summary.id)
        return existing
          ? current.map((conversation) => conversation.id === summary.id ? { ...conversation, ...summary } : conversation)
          : [summary, ...current]
      })
      return true
    } catch (error) {
      if (requestVersion !== historyRequestVersion.current || requestKsId !== activeKsIdRef.current) return false
      const message = (error as Error).message
      setConversationError(message)
      if (notify) toast.error(zh ? `无法加载对话：${message}` : `Could not load conversation: ${message}`)
      return false
    } finally {
      if (requestVersion === historyRequestVersion.current) setHistoryLoading(false)
    }
  }

  useEffect(() => {
    activeRequest.current?.abort()
    activeRequest.current = null
    historyRequestVersion.current += 1
    const listRequestVersion = ++conversationListRequestVersion.current
    renameActionRef.current = "idle"
    setBusy(false)
    setConversationsLoading(false)
    setHistoryLoading(false)
    setRenameSaving(false)
    setDeletingConversationId(null)
    setMessages([])
    setInput("")
    setConversations([])
    setConversationsOpen(false)
    setRenamingConversationId(null)
    setDeleteConfirmConversationId(null)
    setConversationError("")
    if (ksId == null) {
      setConversationId(null)
      return
    }

    const requestKsId = ksId
    const rememberedId = savedConversationId(requestKsId)
    setConversationId(rememberedId)
    let cancelled = false
    setConversationsLoading(true)
    void api.listAgentConversations(requestKsId)
      .then((result) => {
        if (!cancelled && listRequestVersion === conversationListRequestVersion.current) {
          setConversations(result.conversations ?? [])
        }
      })
      .catch((error) => {
        if (!cancelled && listRequestVersion === conversationListRequestVersion.current) {
          setConversationError((error as Error).message)
        }
      })
      .finally(() => {
        if (!cancelled && listRequestVersion === conversationListRequestVersion.current) {
          setConversationsLoading(false)
        }
      })

    if (rememberedId != null) {
      const requestVersion = ++historyRequestVersion.current
      setHistoryLoading(true)
      void api.getAgentConversation(requestKsId, rememberedId)
        .then((detail) => {
          if (cancelled || requestVersion !== historyRequestVersion.current) return
          setMessages(restoreChatMessages(detail.turns ?? [], nextId, nextActivityId))
          setConversations((current) => {
            const summary: AgentConversation = {
              id: detail.id,
              knowledge_system_id: detail.knowledge_system_id,
              title: detail.title,
              first_user_message: detail.first_user_message,
              turn_count: detail.turn_count,
              created_at: detail.created_at,
              updated_at: detail.updated_at,
            }
            return current.some((conversation) => conversation.id === summary.id)
              ? current.map((conversation) => conversation.id === summary.id
                ? { ...conversation, ...summary }
                : conversation)
              : [summary, ...current]
          })
        })
        .catch(() => {
          if (cancelled || requestVersion !== historyRequestVersion.current) return
          rememberConversationId(requestKsId, null)
          setConversationId(null)
          setMessages([])
        })
        .finally(() => {
          if (!cancelled && requestVersion === historyRequestVersion.current) setHistoryLoading(false)
        })
    }

    return () => {
      cancelled = true
    }
  }, [ksId])

  useEffect(() => {
    if (!conversationsOpen) return
    const closeOnPointerDown = (event: PointerEvent) => {
      if (!conversationsMenuRef.current?.contains(event.target as Node)) {
        if (renameActionRef.current !== "saving") renameActionRef.current = "cancelled"
        setConversationsOpen(false)
        setRenamingConversationId(null)
        setRenameDraft("")
        setDeleteConfirmConversationId(null)
      }
    }
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return
      if (renameActionRef.current !== "saving") renameActionRef.current = "cancelled"
      setConversationsOpen(false)
      setRenamingConversationId(null)
      setRenameDraft("")
      setDeleteConfirmConversationId(null)
    }
    document.addEventListener("pointerdown", closeOnPointerDown)
    document.addEventListener("keydown", closeOnEscape)
    return () => {
      document.removeEventListener("pointerdown", closeOnPointerDown)
      document.removeEventListener("keydown", closeOnEscape)
    }
  }, [conversationsOpen])

  const refreshConversations = async (requestKsId = ksId) => {
    if (requestKsId == null || conversationsLoading) return
    const requestVersion = ++conversationListRequestVersion.current
    setConversationsLoading(true)
    setConversationError("")
    try {
      const result = await api.listAgentConversations(requestKsId)
      if (
        requestKsId === activeKsIdRef.current
        && requestVersion === conversationListRequestVersion.current
      ) setConversations(result.conversations ?? [])
    } catch (error) {
      if (
        requestKsId === activeKsIdRef.current
        && requestVersion === conversationListRequestVersion.current
      ) setConversationError((error as Error).message)
    } finally {
      if (
        requestKsId === activeKsIdRef.current
        && requestVersion === conversationListRequestVersion.current
      ) setConversationsLoading(false)
    }
  }

  const toggleConversations = () => {
    const opening = !conversationsOpen
    setConversationsOpen(opening)
    setRenamingConversationId(null)
    setDeleteConfirmConversationId(null)
    if (opening) void refreshConversations()
  }

  const switchConversation = (nextConversationId: number) => {
    if (busy || ksId == null || nextConversationId === conversationId) {
      setConversationsOpen(false)
      return
    }
    historyRequestVersion.current += 1
    setMessages([])
    setInput("")
    setConversationId(nextConversationId)
    rememberConversationId(ksId, nextConversationId)
    setConversationsOpen(false)
    void loadConversation(ksId, nextConversationId)
  }

  const startRenameConversation = (conversation: AgentConversation) => {
    renameActionRef.current = "idle"
    setDeleteConfirmConversationId(null)
    setRenamingConversationId(conversation.id)
    setRenameDraft(displayConversationTitle(conversation, zh))
  }

  const cancelRenameConversation = () => {
    if (renameActionRef.current === "saving") return
    renameActionRef.current = "cancelled"
    setRenamingConversationId(null)
    setRenameDraft("")
  }

  const commitRenameConversation = async (conversation: AgentConversation) => {
    if (
      ksId == null
      || renameSaving
      || renameActionRef.current !== "idle"
      || renamingConversationId !== conversation.id
    ) return
    const title = renameDraft.trim()
    if (!title) {
      cancelRenameConversation()
      return
    }
    const previousTitle = conversation.title?.trim() || conversation.first_user_message?.trim() || ""
    if (title === previousTitle) {
      cancelRenameConversation()
      return
    }
    const requestKsId = ksId
    renameActionRef.current = "saving"
    setRenameSaving(true)
    try {
      const updated = await api.renameAgentConversation(requestKsId, conversation.id, title)
      if (activeKsIdRef.current !== requestKsId) return
      setConversations((current) => current.map((item) => item.id === conversation.id
        ? { ...item, ...updated, title }
        : item))
      setRenamingConversationId(null)
      setRenameDraft("")
    } catch (error) {
      if (activeKsIdRef.current === requestKsId) {
        toast.error(zh ? `重命名失败：${(error as Error).message}` : `Rename failed: ${(error as Error).message}`)
      }
    } finally {
      if (activeKsIdRef.current === requestKsId) {
        renameActionRef.current = "idle"
        setRenameSaving(false)
      }
    }
  }

  const deleteConversation = async (target: AgentConversation) => {
    if (ksId == null || deletingConversationId != null || busy) return
    const requestKsId = ksId
    setDeletingConversationId(target.id)
    try {
      await api.deleteAgentConversation(requestKsId, target.id)
      if (activeKsIdRef.current !== requestKsId) return
      setConversations((current) => current.filter((conversation) => conversation.id !== target.id))
      setDeleteConfirmConversationId(null)
      if (target.id === conversationId) startNewConversation()
    } catch (error) {
      if (activeKsIdRef.current === requestKsId) {
        toast.error(zh ? `删除失败：${(error as Error).message}` : `Delete failed: ${(error as Error).message}`)
      }
    } finally {
      if (activeKsIdRef.current === requestKsId) setDeletingConversationId(null)
    }
  }

  const currentConversation = conversations.find((conversation) => conversation.id === conversationId)
  const currentConversationTitle = displayConversationTitle(currentConversation, zh)
  const isNewConversation = conversationId == null && !historyLoading && messages.length === 0
  const conversationsMenuTitle = isNewConversation ? (zh ? "历史对话" : "Conversations") : currentConversationTitle

  const updateStreamMessage = (messageId: number, event: AgentStreamEvent) => {
    if (event.type === "turn_started") {
      setConversationId(event.conversation_id)
      if (ksId != null) rememberConversationId(ksId, event.conversation_id)
      setConversations((current) => {
        const existing = current.some((conversation) => conversation.id === event.conversation.id)
        return existing
          ? current.map((conversation) => conversation.id === event.conversation.id
            ? { ...conversation, ...event.conversation }
            : conversation)
          : [event.conversation, ...current]
      })
      return
    }
    setMessages((current) => current.map((message) => {
      if (message.id !== messageId || message.role !== "assistant") return message
      if (event.type === "answer_reset") {
        return { ...message, content: "" }
      }
      if (event.type === "progress") {
        // Internal orchestration is intentionally not part of the transcript. Only a
        // real tool that has actually started gets a short-lived visible row.
        if (event.phase !== "tool") return message
        const title = event.title || event.message || (zh ? "正在分析当前信息" : "Analyzing the current information")
        const activity = message.activity ?? []
        const previous = message.activity?.at(-1)
        if (
          previous?.kind === "progress"
          && previous.phase === event.phase
          && previous.title === title
          && previous.detail === event.detail
        ) return message
        const nextProgress: AgentActivity = {
          id: previous?.kind === "progress" ? previous.id : nextActivityId.current++,
          kind: "progress",
          phase: event.phase,
          title,
          detail: event.detail,
        }
        return {
          ...message,
          // A progress event explains the next tool call. Replace adjacent progress
          // updates so the conversation stays text → tool → text → tool.
          activity: previous?.kind === "progress"
            ? [...activity.slice(0, -1), nextProgress]
            : [...activity, nextProgress],
        }
      }
      if (event.type === "commentary") {
        const text = event.text.trim()
        if (!text) return message
        const activity = trimRunningActivity(message.activity) ?? []
        const previous = activity.at(-1)
        if (previous?.kind === "commentary" && previous.text === text) return message
        return {
          ...message,
          activity: [
            ...activity,
            { id: nextActivityId.current++, kind: "commentary" as const, text },
          ],
        }
      }
      if (event.type === "trace") {
        const activity = message.activity ?? []
        const previous = activity.at(-1)
        const completed: AgentActivity = {
          id: previous?.kind === "progress" ? previous.id : nextActivityId.current++,
          kind: "trace",
          trace: event.trace,
        }
        return {
          ...message,
          trace: [...(message.trace ?? []), event.trace],
          activity: previous?.kind === "progress"
            ? [...activity.slice(0, -1), completed]
            : [...activity, completed],
        }
      }
      if (event.type === "delta") {
        return {
          ...message,
          content: message.content + event.delta,
          activity: trimRunningActivity(message.activity),
        }
      }
      if (event.type === "proposal") {
        return { ...message, proposal: event.proposal }
      }
      if (event.type === "error") {
        return {
          ...message,
          streaming: false,
          error: event.message,
          activity: trimRunningActivity(message.activity),
        }
      }

      const hasStreamedTrace = message.activity?.some((activity) => activity.kind === "trace")
      return {
        ...message,
        content: message.content || event.answer,
        trace: event.trace.length ? event.trace : message.trace,
        activity: trimRunningActivity(hasStreamedTrace || !event.trace.length
          ? message.activity
          : [
              ...(message.activity ?? []),
              ...event.trace.map((trace) => ({
                id: nextActivityId.current++,
                kind: "trace" as const,
                trace,
              })),
            ]),
        proposal: event.proposal ?? message.proposal,
        streaming: false,
      }
    }))
  }

  const submit = async () => {
    const content = input.trim()
    if (!content || busy || historyLoading || ksId == null) return
    const requestKsId = ksId
    const requestConversationId = conversationId
    const userMessage: ChatMessage = { id: nextId.current++, role: "user", content }
    const assistantId = nextId.current++
    const controller = new AbortController()
    activeRequest.current = controller
    setMessages((current) => [...current, userMessage, {
      id: assistantId,
      role: "assistant",
      content: "",
      trace: [],
      activity: [],
      proposal: null,
      streaming: true,
    }])
    setInput("")
    setBusy(true)
    try {
      const result = await api.chatWithAgentStream(requestKsId, {
        message: content,
        conversation_id: requestConversationId,
      }, (event) => {
        if (
          activeRequest.current === controller
          && !controller.signal.aborted
          && activeKsIdRef.current === requestKsId
        ) {
          updateStreamMessage(assistantId, event)
        }
      }, controller.signal)
      if (
        activeRequest.current !== controller
        || controller.signal.aborted
        || activeKsIdRef.current !== requestKsId
      ) return
      const resultConversation = result.conversation
      if (resultConversation) {
        setConversationId(resultConversation.id)
        rememberConversationId(requestKsId, resultConversation.id)
        setConversations((current) => {
          const existing = current.some((conversation) => conversation.id === resultConversation.id)
          return existing
            ? current.map((conversation) => conversation.id === resultConversation.id
              ? { ...conversation, ...resultConversation }
              : conversation)
            : [resultConversation, ...current]
        })
      }
      setMessages((current) => current.map((message) => {
        if (message.id !== assistantId) return message
        const hasStreamedTrace = message.activity?.some((activity) => activity.kind === "trace")
        return {
          ...message,
          content: result.answer || message.content,
          trace: result.trace,
          activity: trimRunningActivity(hasStreamedTrace || !result.trace.length
            ? message.activity
            : [
                ...(message.activity ?? []),
                ...result.trace.map((trace) => ({
                  id: nextActivityId.current++,
                  kind: "trace" as const,
                  trace,
                })),
              ]),
          proposal: result.proposal ?? message.proposal,
          streaming: false,
        }
      }))
      void refreshConversations(requestKsId)
    } catch (error) {
      if (
        (error as Error).name === "AbortError"
        || activeRequest.current !== controller
        || activeKsIdRef.current !== requestKsId
      ) return
      const detail = (error as Error).message
      setMessages((current) => current.map((message) => message.id === assistantId
        ? { ...message, streaming: false, error: detail }
        : message))
      toast.error(zh ? `智能体执行失败：${(error as Error).message}` : `Agent failed: ${(error as Error).message}`)
    } finally {
      if (activeRequest.current === controller) {
        activeRequest.current = null
        setBusy(false)
      }
    }
  }

  const keyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault()
      void submit()
    }
  }

  const requestPreview = (messageId: number, proposal: AgentProposal) => {
    onPreviewProposal(proposal)
    setMessages((current) => current.map((message) => message.id === messageId
      ? { ...message, previewRequested: true }
      : message))
  }

  if (!panelPresent) return null

  return (
    <>
      <aside
      id="ontopilot-agent-panel"
      aria-label={zh ? "OntoPilot 智能体" : "OntoPilot agent"}
      aria-hidden={!open}
      inert={!open}
      style={{
        "--agent-panel-left": `${panelAnchor.left}px`,
        "--agent-panel-right": `${panelAnchor.right}px`,
        "--agent-panel-top": `${panelAnchor.top}px`,
        "--agent-arrow-left": `${panelAnchor.arrowLeft}px`,
      } as CSSProperties}
      className={`fixed inset-x-3 top-[var(--agent-panel-top)] z-40 flex h-[min(36rem,calc(100dvh-var(--agent-panel-top)-0.75rem))] min-w-0 max-w-[calc(100vw-1.5rem)] flex-col overflow-visible rounded-2xl border border-border/80 bg-background/95 shadow-[0_24px_70px_-26px_rgba(15,23,42,0.48),0_8px_24px_-14px_rgba(15,23,42,0.28)] backdrop-blur-xl transition-opacity duration-150 ease-out motion-reduce:transition-none md:left-[var(--agent-panel-left)] md:right-[var(--agent-panel-right)] md:h-[min(36rem,calc(100dvh-var(--agent-panel-top)-1.5rem))] md:max-w-[608px] ${panelVisible
        ? "pointer-events-auto opacity-100"
        : "pointer-events-none opacity-0"
      }`}
    >
          <div
            aria-hidden="true"
            className="pointer-events-none absolute top-[-5px] z-10 hidden h-2.5 w-2.5 rotate-45 border-l border-t border-border/80 bg-background md:block"
            style={{ left: "calc(var(--agent-arrow-left) - 5px)" }}
          />
          <header className="flex h-13 shrink-0 items-center gap-2.5 rounded-t-2xl border-b border-border/70 bg-background/90 px-4">
            <Bot className="h-4 w-4 shrink-0 text-primary" />
            <div className="min-w-0 flex-1">
              <h2 className="whitespace-nowrap text-sm font-semibold">OntoPilot Agent</h2>
            </div>
            {!isNewConversation && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-8 shrink-0 gap-1.5 px-2 text-xs"
                onClick={startNewConversation}
                disabled={busy}
                title={zh ? "开始新对话" : "Start a new conversation"}
              >
                <Plus className="h-3.5 w-3.5" />
                <span className="hidden md:inline">{zh ? "新对话" : "New chat"}</span>
              </Button>
            )}
            <div ref={conversationsMenuRef} className="relative min-w-0 max-w-52 flex-1 sm:flex-initial">
              <button
                type="button"
                className={`flex h-8 w-full min-w-0 items-center gap-1.5 rounded-md px-2 text-xs transition-colors hover:bg-muted ${conversationsOpen ? "bg-muted text-foreground" : "text-muted-foreground"}`}
                aria-haspopup="listbox"
                aria-expanded={conversationsOpen}
                onClick={toggleConversations}
              >
                <MessageSquare className="h-3.5 w-3.5 shrink-0" />
                <span className="min-w-0 flex-1 truncate text-left">{conversationsMenuTitle}</span>
                <ChevronDown className={`h-3.5 w-3.5 shrink-0 transition-transform ${conversationsOpen ? "rotate-180" : ""}`} />
              </button>
              {conversationsOpen && (
                <div
                  className="absolute right-0 top-[calc(100%+0.4rem)] z-50 w-[min(22rem,calc(100vw-2rem))] overflow-hidden rounded-xl border border-border/80 bg-popover p-1.5 text-popover-foreground shadow-md"
                  role="listbox"
                  aria-label={zh ? "历史对话" : "Conversation history"}
                >
                  <div className="flex items-center justify-between gap-2 px-2 py-1.5">
                    <span className="text-[11px] font-medium text-muted-foreground">{zh ? "历史对话" : "Conversations"}</span>
                    {conversationsLoading && <Loader2 className="h-3.5 w-3.5 animate-spin text-primary" />}
                  </div>
                  {conversationError && !conversationsLoading && (
                    <p className="mx-1 mb-1 rounded-md bg-destructive/8 px-2 py-1.5 text-[11px] leading-relaxed text-destructive">
                      {zh ? "暂时无法加载历史对话" : "Conversation history is temporarily unavailable"}
                    </p>
                  )}
                  {!conversationsLoading && conversations.length === 0 ? (
                    <p className="px-3 py-5 text-center text-xs text-muted-foreground">
                      {zh ? "还没有历史对话" : "No previous conversations"}
                    </p>
                  ) : (
                    <div className="max-h-72 space-y-0.5 overflow-y-auto">
                      {conversations.map((conversation) => {
                        const isCurrent = conversation.id === conversationId
                        const confirmingDelete = deleteConfirmConversationId === conversation.id
                        const deleting = deletingConversationId === conversation.id
                        return (
                          <div
                            key={conversation.id}
                            role="option"
                            aria-selected={isCurrent}
                            className={`group flex min-w-0 items-center gap-1 rounded-md px-1 py-0.5 transition-colors ${isCurrent
                              ? "bg-muted/70 text-foreground"
                              : "text-muted-foreground hover:bg-muted/40 hover:text-foreground"
                            }`}
                          >
                            {renamingConversationId === conversation.id ? (
                              <Input
                                autoFocus
                                value={renameDraft}
                                maxLength={120}
                                disabled={renameSaving}
                                className="h-8 min-w-0 flex-1 px-2 text-xs"
                                aria-label={zh ? "对话标题" : "Conversation title"}
                                onChange={(event) => setRenameDraft(event.target.value)}
                                onClick={(event) => event.stopPropagation()}
                                onBlur={() => void commitRenameConversation(conversation)}
                                onKeyDown={(event) => {
                                  if (event.key === "Enter") {
                                    event.preventDefault()
                                    void commitRenameConversation(conversation)
                                  }
                                  if (event.key === "Escape") {
                                    event.preventDefault()
                                    cancelRenameConversation()
                                  }
                                }}
                              />
                            ) : (
                              <button
                                type="button"
                                className="min-w-0 flex-1 rounded-md px-2 py-1.5 text-left"
                                onClick={() => switchConversation(conversation.id)}
                                disabled={busy || deleting}
                              >
                                <span className="block truncate text-xs font-medium">
                                  {displayConversationTitle(conversation, zh)}
                                </span>
                              </button>
                            )}
                            {renamingConversationId !== conversation.id && (
                              confirmingDelete ? (
                                <div className="flex shrink-0 items-center gap-1 pr-1">
                                  <button
                                    type="button"
                                    className="rounded px-1.5 py-1 text-[10px] text-muted-foreground hover:bg-background"
                                    onClick={() => setDeleteConfirmConversationId(null)}
                                  >
                                    {zh ? "取消" : "Cancel"}
                                  </button>
                                  <button
                                    type="button"
                                    className="rounded bg-destructive px-1.5 py-1 text-[10px] text-destructive-foreground hover:bg-destructive/90"
                                    disabled={deleting}
                                    onClick={() => void deleteConversation(conversation)}
                                  >
                                    {deleting ? <Loader2 className="h-3 w-3 animate-spin" /> : (zh ? "删除" : "Delete")}
                                  </button>
                                </div>
                              ) : (
                                <div className="flex shrink-0 items-center pr-1 transition-opacity sm:pointer-events-none sm:opacity-0 sm:group-hover:pointer-events-auto sm:group-hover:opacity-100 sm:group-focus-within:pointer-events-auto sm:group-focus-within:opacity-100">
                                  <button
                                    type="button"
                                    className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
                                    aria-label={zh ? "重命名" : "Rename"}
                                    title={zh ? "重命名" : "Rename"}
                                    onClick={() => startRenameConversation(conversation)}
                                  >
                                    <Pencil className="h-3.5 w-3.5" />
                                  </button>
                                  <button
                                    type="button"
                                    className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                                    aria-label={zh ? "删除" : "Delete"}
                                    title={zh ? "删除" : "Delete"}
                                    onClick={() => {
                                      setRenamingConversationId(null)
                                      setDeleteConfirmConversationId(conversation.id)
                                    }}
                                  >
                                    <Trash2 className="h-3.5 w-3.5" />
                                  </button>
                                </div>
                              )
                            )}
                          </div>
                        )
                      })}
                    </div>
                  )}
                </div>
              )}
            </div>
            <Button type="button" variant="ghost" size="icon" className="h-8 w-8" onClick={() => onOpenChange(false)} title={zh ? "关闭" : "Close"}>
              <X className="h-4 w-4" />
            </Button>
          </header>

          <div ref={conversationRef} className="min-h-0 min-w-0 flex-1 overflow-x-hidden overflow-y-auto px-4 sm:px-5">
            <div className="mx-auto w-full min-w-0 max-w-[33rem] space-y-4 overflow-hidden pb-20 pt-4">
              {ksId == null && (
                <div className="mx-auto flex min-h-80 max-w-xl flex-col justify-center py-8">
                  <div className="mb-5">
                    <p className="text-base font-semibold tracking-tight">
                      {zh ? "选择一个知识体系开始" : "Choose a knowledge system to begin"}
                    </p>
                    <p className="mt-1.5 text-sm leading-relaxed text-muted-foreground">
                      {zh
                        ? "智能体只会读取你选择的知识体系，不会把首页卡片、搜索条件或其他页面信息加入对话。"
                        : "The agent only reads the knowledge system you choose. Home-page cards, filters, and other page state are not added to the conversation."}
                    </p>
                  </div>

                  {knowledgeSystemsLoading ? (
                    <div className="flex items-center gap-2 rounded-xl border border-dashed px-4 py-5 text-sm text-muted-foreground">
                      <Loader2 className="h-4 w-4 animate-spin text-primary" />
                      {zh ? "正在加载知识体系…" : "Loading knowledge systems…"}
                    </div>
                  ) : knowledgeSystems?.length ? (
                    <div className="grid gap-2 sm:grid-cols-2">
                      {knowledgeSystems.map((system) => (
                        <button
                          key={system.id}
                          type="button"
                          className="min-w-0 rounded-xl border bg-background p-3 text-left transition-colors hover:border-primary/40 hover:bg-primary/[0.04] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                          onClick={() => onKnowledgeSystemChange?.(system.id)}
                        >
                          <div className="flex min-w-0 items-center justify-between gap-2">
                            <span className="truncate text-sm font-medium">{system.name}</span>
                            <Badge variant="outline" className="shrink-0 text-[10px]">
                              {system.my_role === "owner"
                                ? (zh ? "所有者" : "Owner")
                                : system.my_role === "editor"
                                  ? (zh ? "编辑者" : "Editor")
                                  : (zh ? "查看者" : "Viewer")}
                            </Badge>
                          </div>
                          <p className="mt-1 line-clamp-2 min-h-8 text-xs leading-relaxed text-muted-foreground">
                            {system.description || (zh ? "暂无描述" : "No description")}
                          </p>
                          <p className="mt-2 text-[10px] text-muted-foreground">
                            {zh
                              ? `${system.class_count} 个类 · ${system.property_count} 个属性`
                              : `${system.class_count} classes · ${system.property_count} properties`}
                          </p>
                        </button>
                      ))}
                    </div>
                  ) : (
                    <div className="rounded-xl border border-dashed px-4 py-5 text-sm text-muted-foreground">
                      {zh ? "还没有可用的知识体系，请先在首页创建一个。" : "No knowledge systems are available yet. Create one on the home page first."}
                    </div>
                  )}
                </div>
              )}

              {ksId != null && historyLoading && (
                <div className="flex min-h-72 items-center justify-center gap-2 text-sm text-muted-foreground">
                  <Loader2 className="h-4 w-4 animate-spin text-primary" />
                  {zh ? "正在加载对话…" : "Loading conversation…"}
                </div>
              )}

              {ksId != null && !historyLoading && messages.length === 0 && (
                <div className="mx-auto flex min-h-72 w-full max-w-[35rem] flex-col justify-center py-6">
                  <div className="min-w-0">
                    <div>
                      <p className="text-base font-semibold tracking-tight">{zh ? "使用自然语言治理本体" : "Govern your ontology in natural language"}</p>
                      <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
                        {zh
                          ? "描述你的目标，智能体会读取当前本体上下文、检索关联知识节点，并在写入前展示工具调用与变更预览。"
                          : "Describe the outcome you want. The agent reads the current ontology context, searches related knowledge nodes, and shows tool calls and a change preview before writing."}
                      </p>
                    </div>
                    <div className="mt-5 divide-y border-y">
                      {(zh
                        ? ["检查当前本体中的结构问题", "补充设备与维护策略的关系", "给出最值得优先处理的治理建议"]
                        : ["Check this ontology for structural issues", "Add equipment-to-maintenance relationships", "Suggest the highest-priority governance improvement"]
                      ).map((example) => (
                        <button
                          key={example}
                          type="button"
                          className="flex min-h-11 w-full items-center py-3 text-left text-xs leading-relaxed text-muted-foreground transition-colors hover:text-foreground"
                          onClick={() => setInput(example)}
                        >
                          {example}
                        </button>
                      ))}
                    </div>
                  </div>
                </div>
              )}

              {messages.map((message) => (
                <div
                  key={message.id}
                  aria-busy={message.role === "assistant" ? message.streaming : undefined}
                  aria-live={message.role === "assistant" ? (message.streaming ? "off" : "polite") : undefined}
                  className={message.role === "user" ? "flex min-w-0 max-w-full justify-end" : "min-w-0 max-w-full space-y-2 overflow-hidden"}
                >
                  {message.role === "user" ? (
                    <div className="min-w-0 max-w-[88%] whitespace-pre-wrap break-words rounded-xl rounded-br-sm bg-primary px-3 py-2 text-sm text-primary-foreground [overflow-wrap:anywhere]">
                      {message.content}
                    </div>
                  ) : (
                    <>
                      {message.activity && message.activity.length > 0 && (
                        <div className="w-full min-w-0 max-w-full space-y-1.5">
                          {groupAdjacentToolActivity(message.activity).map((activity) => {
                            if (activity.kind === "progress") {
                              const isCurrent = Boolean(message.streaming && activity.id === message.activity?.at(-1)?.id)
                              if (isCurrent) {
                                return (
                                  <RunningStatus
                                    key={activity.id}
                                    label={activity.title}
                                    framed
                                  />
                                )
                              }
                              return (
                                <div key={activity.id} className="flex min-w-0 items-center gap-2 rounded-md bg-muted/35 px-2.5 py-2 text-xs text-muted-foreground">
                                  <span className="min-w-0 truncate">{activity.title}</span>
                                </div>
                              )
                            }
                            if (activity.kind === "commentary") {
                              return (
                                <p key={activity.id} className="px-1 text-sm leading-6 text-foreground/85">
                                  {activity.text}
                                </p>
                              )
                            }
                            const traces = activity.traces
                            const firstTrace = traces[0]
                            return (
                              <details key={activity.id} className="group min-w-0 overflow-hidden rounded-md border border-border/70 bg-background">
                                <summary className="flex min-w-0 cursor-pointer list-none items-center gap-2 px-2.5 py-2 text-xs marker:content-none">
                                  <Wrench className="h-3.5 w-3.5 shrink-0 text-primary" />
                                  <span className="shrink-0 font-medium">{readableToolName(firstTrace.tool, zh)}</span>
                                  <span className="min-w-0 flex-1 truncate text-muted-foreground">{groupedTraceSummary(traces, zh)}</span>
                                </summary>
                                <div className="min-w-0 border-t bg-muted/20 px-3 py-2">
                                  {traces.map((trace, index) => (
                                    <div
                                      key={`${activity.id}-${index}`}
                                      className={index === 0 ? "min-w-0" : "mt-2 min-w-0 border-t border-border/60 pt-2"}
                                    >
                                      <div className="mb-1 flex min-w-0 items-center gap-2 text-[10px]">
                                        {traces.length > 1 && (
                                          <span className="shrink-0 font-mono text-muted-foreground/65">#{index + 1}</span>
                                        )}
                                        <code className="min-w-0 flex-1 truncate font-medium text-foreground">{trace.tool}</code>
                                        {traces.length > 1 && (
                                          <span className="min-w-0 truncate text-muted-foreground">{trace.summary}</span>
                                        )}
                                      </div>
                                      <code className="block break-all text-[10px] leading-relaxed text-muted-foreground">
                                        {compactToolArguments(trace.arguments)}
                                      </code>
                                    </div>
                                  ))}
                                </div>
                              </details>
                            )
                          })}
                        </div>
                      )}
                      {message.streaming && !message.content && message.activity?.at(-1)?.kind !== "progress" && (
                        <RunningStatus
                          label={runningLabel(message, zh)}
                        />
                      )}
                      {message.content && (
                        <div className="w-full min-w-0 max-w-full overflow-hidden">
                          <AgentMarkdown source={message.content} />
                          {message.streaming && <span aria-hidden="true" className="ml-0.5 inline-block h-3.5 w-0.5 animate-pulse bg-primary align-middle" />}
                        </div>
                      )}
                      {message.error && (
                        <div role="alert" className="flex min-w-0 max-w-full items-start gap-2 rounded-lg bg-destructive/8 px-3 py-2 text-xs text-destructive">
                          <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                          <span className="min-w-0 break-words [overflow-wrap:anywhere]">
                            {zh ? `执行中断：${message.error}` : `The run stopped: ${message.error}`}
                          </span>
                        </div>
                      )}
                      {message.proposal && (
                        <div className="pt-2">
                          <div className="w-fit min-w-0 max-w-full overflow-hidden rounded-lg border bg-muted/20 p-3 sm:max-w-[26rem]">
                            <div className="flex min-w-0 items-start gap-2">
                              <Sparkles className="mt-0.5 h-4 w-4 shrink-0 text-primary" />
                              <div className="min-w-0 flex-1 break-words [overflow-wrap:anywhere]">
                                <p className="text-sm font-medium">{zh ? "智能体建议" : "Agent suggestion"}</p>
                                <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                                  {message.proposal.reason || message.proposal.summary}
                                  {zh
                                    ? ` 修改了 ${message.proposal.operations.length} 处，已通过校验。`
                                    : ` ${message.proposal.operations.length} change${message.proposal.operations.length === 1 ? "" : "s"}, validation passed.`}
                                </p>
                                <Button
                                  type="button"
                                  size="sm"
                                  className="mt-3 h-7 px-2.5 text-xs"
                                  variant={message.previewRequested ? "outline" : "default"}
                                  disabled={!canWrite}
                                  onClick={() => requestPreview(message.id, message.proposal!)}
                                >
                                  {message.previewRequested
                                    ? (zh ? "再次查看" : "Review again")
                                    : canWrite
                                      ? (zh ? "查看修改" : "Review changes")
                                      : (zh ? "只读" : "Read only")}
                                </Button>
                              </div>
                            </div>
                          </div>
                        </div>
                      )}
                    </>
                  )}
                </div>
              ))}

            </div>
          </div>

          <div className="relative shrink-0 rounded-b-2xl bg-background/90 px-4 pb-3 sm:px-5 sm:pb-4">
            {knowledgeSystems && onKnowledgeSystemChange && (
              <div className="absolute inset-x-4 bottom-full mb-2 sm:inset-x-5">
                <div className="mx-auto flex min-w-0 max-w-[35rem] items-center">
                  <Badge asChild variant="secondary" className="h-6 max-w-full rounded-md border-0 bg-muted/75 px-2 font-normal text-foreground shadow-none hover:bg-muted">
                    <button
                      type="button"
                      disabled={busy || knowledgeSystemsLoading || knowledgeSystems.length === 0}
                      onClick={() => {
                        setKnowledgeSearch("")
                        setKnowledgePickerOpen(true)
                      }}
                      aria-haspopup="dialog"
                      aria-expanded={knowledgePickerOpen}
                      aria-label={zh ? "选择知识体系" : "Select knowledge system"}
                    >
                      <span className="truncate">
                        {knowledgeSystemsLoading
                          ? (zh ? "正在加载知识体系…" : "Loading knowledge systems…")
                          : selectedKnowledgeSystem?.name ?? (zh ? "选择知识体系" : "Select a knowledge system")}
                      </span>
                      <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
                    </button>
                  </Badge>
                </div>
              </div>
            )}
            <div className="mx-auto max-w-[35rem]">
              <div className="relative">
                <Textarea
                  value={input}
                  onChange={(event) => setInput(event.target.value)}
                  onKeyDown={keyDown}
                  disabled={busy || historyLoading || ksId == null}
                  className="max-h-36 min-h-[68px] resize-none rounded-xl bg-muted/20 pr-11 shadow-none focus-visible:bg-background"
                  placeholder={ksId == null
                    ? (zh ? "请先选择知识体系" : "Select a knowledge system first")
                    : zh
                      ? "询问当前知识体系，或描述你想完成的修改…"
                      : "Ask about this knowledge system or describe a change…"}
                />
                <Button type="button" size="icon" className="absolute bottom-2 right-2 h-8 w-8" disabled={ksId == null || !input.trim() || busy || historyLoading} onClick={() => void submit()} aria-label={zh ? "发送" : "Send"}>
                  {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Send className="h-3.5 w-3.5" />}
                </Button>
              </div>
            </div>
            <p className="mx-auto mt-1.5 max-w-[33rem] -translate-x-px text-[10px] text-muted-foreground">
              {zh ? "Enter 发送 · Shift+Enter 换行" : "Enter to send · Shift+Enter for a new line"}
            </p>
          </div>
      </aside>

      {knowledgePickerOpen && knowledgeSystems && onKnowledgeSystemChange && createPortal(
        <div
          className="fixed inset-0 z-[100] flex items-center justify-center bg-black/30 p-4 backdrop-blur-[2px]"
          role="dialog"
          aria-modal="true"
          aria-labelledby="knowledge-picker-title"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) setKnowledgePickerOpen(false)
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape") setKnowledgePickerOpen(false)
          }}
        >
          <div className="flex max-h-[min(70svh,40rem)] w-full max-w-xl flex-col overflow-hidden rounded-2xl bg-background shadow-2xl ring-1 ring-foreground/10">
            <div className="flex items-center justify-between px-5 pb-3 pt-4">
              <h2 id="knowledge-picker-title" className="text-sm font-semibold">
                {zh ? "选择知识体系" : "Choose a knowledge system"}
              </h2>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-8 w-8"
                onClick={() => setKnowledgePickerOpen(false)}
                aria-label={zh ? "关闭" : "Close"}
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
            <div className="px-5 pb-3">
              <div className="relative">
                <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  autoFocus
                  value={knowledgeSearch}
                  onChange={(event) => setKnowledgeSearch(event.target.value)}
                  className="h-10 rounded-xl bg-muted/35 pl-9 shadow-none"
                  placeholder={zh ? "搜索知识体系…" : "Search knowledge systems…"}
                />
              </div>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
              {visibleKnowledgeSystems.length > 0 ? (
                <div className="space-y-0.5">
                  {visibleKnowledgeSystems.map((system) => {
                    const selected = system.id === ksId
                    return (
                      <button
                        key={system.id}
                        type="button"
                        className={`flex w-full min-w-0 items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm transition-colors ${selected
                          ? "bg-muted text-foreground"
                          : "text-foreground/85 hover:bg-muted/60 hover:text-foreground"
                        }`}
                        onClick={() => {
                          onKnowledgeSystemChange(system.id)
                          setKnowledgePickerOpen(false)
                          setKnowledgeSearch("")
                        }}
                      >
                        <span className="min-w-0 flex-1 truncate">{system.name}</span>
                        {selected && <Check className="h-4 w-4 shrink-0 text-primary" />}
                      </button>
                    )
                  })}
                </div>
              ) : (
                <p className="px-3 py-10 text-center text-sm text-muted-foreground">
                  {zh ? "没有匹配的知识体系" : "No matching knowledge systems"}
                </p>
              )}
            </div>
          </div>
        </div>,
        document.body,
      )}
    </>
  )
}
