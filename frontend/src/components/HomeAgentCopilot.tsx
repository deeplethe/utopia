import { useEffect, useMemo, useState } from "react"
import { useNavigate } from "react-router-dom"
import { toast } from "sonner"
import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import type { AgentProposal, KnowledgeSystem } from "@/lib/types"
import AgentCopilot from "@/components/AgentCopilot"

type HomeAgentCopilotProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  onBusyChange: (busy: boolean) => void
}

const HOME_AGENT_KS_KEY = "ontopilot:agent:home-knowledge-system"

function rememberedKnowledgeSystemId() {
  try {
    const value = Number(localStorage.getItem(HOME_AGENT_KS_KEY))
    return Number.isInteger(value) && value > 0 ? value : null
  } catch {
    return null
  }
}

function rememberKnowledgeSystemId(ksId: number | null) {
  try {
    if (ksId == null) localStorage.removeItem(HOME_AGENT_KS_KEY)
    else localStorage.setItem(HOME_AGENT_KS_KEY, String(ksId))
  } catch {
    // Private browsing or a hardened browser may block localStorage.
  }
}

export default function HomeAgentCopilot({ open, onOpenChange, onBusyChange }: HomeAgentCopilotProps) {
  const { locale } = useI18n()
  const navigate = useNavigate()
  const [systems, setSystems] = useState<KnowledgeSystem[]>([])
  const [selectedKsId, setSelectedKsId] = useState<number | null>(null)
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (!open) return
    let cancelled = false
    setLoading(true)
    api.listKS()
      .then((nextSystems) => {
        if (cancelled) return
        setSystems(nextSystems)
        setSelectedKsId((current) => {
          const candidate = current ?? rememberedKnowledgeSystemId()
          const next = candidate != null && nextSystems.some((system) => system.id === candidate)
            ? candidate
            : null
          rememberKnowledgeSystemId(next)
          return next
        })
      })
      .catch((error: Error) => {
        if (cancelled) return
        toast.error(locale === "zh-CN"
          ? `加载知识体系失败：${error.message}`
          : `Failed to load knowledge systems: ${error.message}`)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [locale, open])

  const selectedSystem = useMemo(
    () => systems.find((system) => system.id === selectedKsId) ?? null,
    [selectedKsId, systems],
  )

  const previewProposal = (proposal: AgentProposal) => {
    if (!selectedSystem) return
    onOpenChange(false)
    navigate(`/knowledge/${selectedSystem.id}/ontology`, { state: { agentProposal: proposal } })
  }

  const selectKnowledgeSystem = (ksId: number | null) => {
    rememberKnowledgeSystemId(ksId)
    setSelectedKsId(ksId)
  }

  return (
    <AgentCopilot
      key={selectedKsId ?? "select-knowledge-system"}
      open={open}
      onOpenChange={onOpenChange}
      onBusyChange={onBusyChange}
      ksId={selectedKsId}
      canWrite={selectedSystem?.my_role === "owner" || selectedSystem?.my_role === "editor"}
      onPreviewProposal={previewProposal}
      knowledgeSystems={systems}
      knowledgeSystemsLoading={loading}
      onKnowledgeSystemChange={selectKnowledgeSystem}
    />
  )
}
