import { useCallback, useEffect, useMemo, useState } from "react"
import { Loader2, RotateCcw, Save, Search, Undo2 } from "lucide-react"
import { toast } from "sonner"

import { api } from "@/lib/api"
import { useI18n, type MessageKey, type Translate } from "@/lib/i18n"
import { useConfirm } from "@/lib/confirm"
import type { KnowledgePrompt, PromptCategory } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Textarea } from "@/components/ui/textarea"

const PROMPT_META: Record<string, { title: MessageKey; description: MessageKey }> = {
  "tbox.extract.rag": { title: "prompts.meta.tboxExtract.title", description: "prompts.meta.tboxExtract.description" },
  "tbox.extract.agent": { title: "prompts.meta.tboxAgent.title", description: "prompts.meta.tboxAgent.description" },
  "tbox.hierarchy.recovery": { title: "prompts.meta.hierarchyRecovery.title", description: "prompts.meta.hierarchyRecovery.description" },
  "abox.extract": { title: "prompts.meta.aboxExtract.title", description: "prompts.meta.aboxExtract.description" },
  "tbox.boundary.critic": { title: "prompts.meta.tboxCritic.title", description: "prompts.meta.tboxCritic.description" },
  "tbox.boundary.adjudicator": { title: "prompts.meta.tboxAdjudicator.title", description: "prompts.meta.tboxAdjudicator.description" },
  "tbox.boundary.evidence_selector": { title: "prompts.meta.evidenceSelector.title", description: "prompts.meta.evidenceSelector.description" },
  "tbox.boundary.corpus_recovery": { title: "prompts.meta.corpusRecovery.title", description: "prompts.meta.corpusRecovery.description" },
  "tbox.denotation.critic": { title: "prompts.meta.tboxDenotation.title", description: "prompts.meta.tboxDenotation.description" },
  "abox.boundary.critic": { title: "prompts.meta.aboxCritic.title", description: "prompts.meta.aboxCritic.description" },
  "abox.boundary.self_typed_adjudicator": { title: "prompts.meta.selfTypedAdjudicator.title", description: "prompts.meta.selfTypedAdjudicator.description" },
  "tbox.hierarchy.critic": { title: "prompts.meta.subclassCritic.title", description: "prompts.meta.subclassCritic.description" },
  "abox.entity_resolution": { title: "prompts.meta.entityResolution.title", description: "prompts.meta.entityResolution.description" },
  "conflict.duplicate_judge": { title: "prompts.meta.duplicateJudge.title", description: "prompts.meta.duplicateJudge.description" },
  "conflict.resolution": { title: "prompts.meta.conflictResolution.title", description: "prompts.meta.conflictResolution.description" },
  "tbox.structure_repair": { title: "prompts.meta.structureRepair.title", description: "prompts.meta.structureRepair.description" },
  "tbox.domain_range_reconcile": { title: "prompts.meta.reconcile.title", description: "prompts.meta.reconcile.description" },
  "terminology.steward": { title: "prompts.meta.terminology.title", description: "prompts.meta.terminology.description" },
  "abox.datatype_validation": { title: "prompts.meta.validation.title", description: "prompts.meta.validation.description" },
}

const CATEGORY_KEYS: Record<PromptCategory, MessageKey> = {
  extraction: "prompts.category.extraction",
  review: "prompts.category.review",
  governance: "prompts.category.governance",
  validation: "prompts.category.validation",
}

function titleOf(prompt: KnowledgePrompt, t: Translate) {
  const meta = PROMPT_META[prompt.key]
  return meta ? t(meta.title) : prompt.title
}

export default function PromptSettingsPanel({
  ksId,
  canWrite,
}: {
  ksId: number
  canWrite: boolean
}) {
  const { locale, t } = useI18n()
  const confirmAction = useConfirm()
  const [items, setItems] = useState<KnowledgePrompt[]>([])
  const [selectedKey, setSelectedKey] = useState("")
  const [draft, setDraft] = useState("")
  const [query, setQuery] = useState("")
  const [category, setCategory] = useState<"all" | PromptCategory>("all")
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [restoringAll, setRestoringAll] = useState(false)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const response = await api.listPrompts(ksId)
      setItems(response.items)
      setSelectedKey((current) => (
        current && response.items.some((item) => item.key === current)
          ? current
          : response.items[0]?.key ?? ""
      ))
    } catch (error) {
      toast.error(t("prompts.loadFailed", { error: (error as Error).message }))
    } finally {
      setLoading(false)
    }
  }, [ksId, t])

  useEffect(() => { load() }, [load])

  const selected = items.find((item) => item.key === selectedKey) ?? null
  useEffect(() => {
    setDraft(selected?.effective_content ?? "")
  }, [selected?.key, selected?.effective_content])

  const dirty = selected != null && draft !== selected.effective_content
  const customCount = items.filter((item) => item.is_overridden).length
  const filtered = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase(locale)
    return items.filter((item) => {
      if (category !== "all" && item.category !== category) return false
      if (!normalized) return true
      return [titleOf(item, t), item.key]
        .some((value) => value.toLocaleLowerCase(locale).includes(normalized))
    })
  }, [category, items, locale, query, t])

  useEffect(() => {
    if (!dirty) return
    const warn = (event: BeforeUnloadEvent) => event.preventDefault()
    window.addEventListener("beforeunload", warn)
    return () => window.removeEventListener("beforeunload", warn)
  }, [dirty])

  const choose = async (key: string) => {
    if (key === selectedKey) return
    if (dirty && !await confirmAction(t("prompts.discardConfirm"))) return
    setSelectedKey(key)
  }

  const replaceItem = (next: KnowledgePrompt) => {
    setItems((current) => current.map((item) => item.key === next.key ? next : item))
  }

  const save = async () => {
    if (!selected || !dirty) return
    setSaving(true)
    try {
      const updated = await api.updatePrompt(ksId, selected.key, draft)
      replaceItem(updated)
      setDraft(updated.effective_content)
      toast.success(t("prompts.saved"))
    } catch (error) {
      toast.error(t("prompts.saveFailed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setSaving(false)
    }
  }

  const restore = async () => {
    if (!selected || !selected.is_overridden) return
    if (!await confirmAction(t("prompts.restoreConfirm", { name: titleOf(selected, t) }), { destructive: true })) return
    setSaving(true)
    try {
      const updated = await api.restorePrompt(ksId, selected.key)
      replaceItem(updated)
      setDraft(updated.effective_content)
      toast.success(t("prompts.restored"))
    } catch (error) {
      toast.error(t("prompts.restoreFailed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setSaving(false)
    }
  }

  const restoreAll = async () => {
    if (!customCount || !await confirmAction(t("prompts.restoreAllConfirm", { count: customCount }), { destructive: true })) return
    setRestoringAll(true)
    try {
      await api.restoreAllPrompts(ksId)
      setItems((current) => current.map((item) => ({
        ...item,
        effective_content: item.default_content,
        is_overridden: false,
        updated_at: null,
        updated_by: null,
      })))
      if (selected) setDraft(selected.default_content)
      toast.success(t("prompts.restoreAllSuccess"))
    } catch (error) {
      toast.error(t("prompts.restoreFailed", { error: (error as Error).message.replace(/^\d+:\s*/, "") }))
    } finally {
      setRestoringAll(false)
    }
  }

  if (loading) {
    return (
      <div className="flex h-64 items-center justify-center text-sm text-muted-foreground">
        <Loader2 className="mr-2 h-4 w-4 animate-spin" /> {t("common.loading")}
      </div>
    )
  }

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">{t("prompts.title")}</h2>
        <div className="flex flex-wrap items-center justify-end gap-2">
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("prompts.search")}
              className="h-8 w-52 pl-7 text-sm"
            />
          </div>
          <Select value={category} onValueChange={(value) => setCategory(value as "all" | PromptCategory)}>
            <SelectTrigger className="h-8 w-36 text-sm"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t("prompts.allCategories")}</SelectItem>
              {(Object.keys(CATEGORY_KEYS) as PromptCategory[]).map((value) => (
                <SelectItem key={value} value={value}>{t(CATEGORY_KEYS[value])}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          {canWrite && customCount > 0 && (
            <Button variant="outline" size="sm" onClick={restoreAll} disabled={restoringAll || saving}>
              {restoringAll
                ? <Loader2 className="h-4 w-4 animate-spin" />
                : <RotateCcw className="h-4 w-4" />}
              {t("prompts.restoreAll", { count: customCount })}
            </Button>
          )}
        </div>
      </div>

      <div className="grid min-h-[640px] overflow-hidden rounded-lg border bg-background lg:grid-cols-[260px_minmax(0,1fr)]">
        <aside className="min-h-0 border-b lg:border-b-0 lg:border-r">
          <div className="max-h-[720px] overflow-y-auto p-2">
            {filtered.length === 0 ? (
              <p className="px-3 py-8 text-center text-xs text-muted-foreground">{t("prompts.noMatches")}</p>
            ) : filtered.map((item) => (
              <button
                key={item.key}
                type="button"
                onClick={() => choose(item.key)}
                className={`mb-0.5 w-full rounded-md px-3 py-2.5 text-left transition-colors ${
                  selectedKey === item.key
                    ? "bg-muted"
                    : "hover:bg-muted/60"
                }`}
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate text-sm font-medium leading-5">{titleOf(item, t)}</span>
                  {item.is_overridden && (
                    <span
                      className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-500"
                      title={t("prompts.custom")}
                    />
                  )}
                </div>
              </button>
            ))}
          </div>
        </aside>

        <section className="flex min-w-0 flex-col">
          {selected ? (
            <>
              <div className="border-b px-4 py-3">
                <h3 className="text-base font-semibold">{titleOf(selected, t)}</h3>
                {selected.variables.length > 0 && (
                  <div className="mt-1.5 text-[11px] text-muted-foreground">
                    {t("prompts.requiredVariables")}: {selected.variables.map((name) => (
                      <code key={name} className="ml-1 rounded bg-muted px-1 py-0.5">{`{${name}}`}</code>
                    ))}
                  </div>
                )}
              </div>

              <div className="flex-1 space-y-3 p-4">
                <Textarea
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  readOnly={!canWrite}
                  spellCheck={false}
                  className="min-h-[470px] resize-y whitespace-pre-wrap font-mono text-xs leading-5"
                  aria-label={titleOf(selected, t)}
                />

                <div className="flex flex-wrap items-center justify-between gap-3 text-[11px] text-muted-foreground">
                  <span>{t("prompts.characters", { count: draft.length.toLocaleString(locale) })}</span>
                  <span>
                    {selected.updated_at
                      ? t("prompts.lastUpdated", {
                          user: selected.updated_by ?? t("common.agent"),
                          time: new Date(selected.updated_at).toLocaleString(locale),
                        })
                      : t("prompts.neverCustomized")}
                  </span>
                </div>

                {selected.is_overridden && (
                  <details className="rounded-lg border bg-muted/15">
                    <summary className="cursor-pointer px-3 py-2 text-xs font-medium">{t("prompts.defaultPreview")}</summary>
                    <pre className="max-h-64 overflow-auto border-t p-3 whitespace-pre-wrap text-[11px] leading-5 text-muted-foreground">
                      {selected.default_content}
                    </pre>
                  </details>
                )}
              </div>

              <div className="flex flex-wrap items-center justify-between gap-3 border-t px-4 py-3">
                <p className="text-xs text-muted-foreground">
                  {canWrite ? (dirty ? t("prompts.unsaved") : t("prompts.savedState")) : t("prompts.readOnly")}
                </p>
                {canWrite && (
                  <div className="flex flex-wrap gap-2">
                    {selected.is_overridden && (
                      <Button variant="outline" size="sm" onClick={restore} disabled={saving}>
                        <RotateCcw className="h-4 w-4" /> {t("prompts.restoreDefault")}
                      </Button>
                    )}
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => setDraft(selected.effective_content)}
                      disabled={!dirty || saving}
                    >
                      <Undo2 className="h-4 w-4" /> {t("prompts.resetDraft")}
                    </Button>
                    <Button size="sm" onClick={save} disabled={!dirty || saving || !draft.trim()}>
                      {saving ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
                      {t("common.save")}
                    </Button>
                  </div>
                )}
              </div>
            </>
          ) : (
            <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
              {t("prompts.select")}
            </div>
          )}
        </section>
      </div>
    </div>
  )
}
