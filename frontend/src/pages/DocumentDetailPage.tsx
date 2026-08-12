import { useEffect, useMemo, useRef, useState } from "react"
import { Link, Navigate, useLocation, useParams } from "react-router-dom"
import { ArrowLeft, CheckCircle2, FileText, Loader2, Search } from "lucide-react"

import { api } from "@/lib/api"
import { useI18n } from "@/lib/i18n"
import type { Chunk, DocumentContribution, DocumentMeta } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ReviewPagination } from "@/components/review-bits"

const CHUNK_PAGE_SIZE = 10

type TextMatch = {
  index: number
  start: number
  end: number
  chunkPosition: number
}

function escapeSearchTerm(value: string): string {
  const specialCharacters = "\\^$.*+?()[]{}|"
  return [...value]
    .map((character) => specialCharacters.includes(character) ? `\\${character}` : character)
    .join("")
}

function HighlightedChunkText({
  text,
  matches,
  activeMatchIndex,
  registerMatch,
}: {
  text: string
  matches: TextMatch[]
  activeMatchIndex: number
  registerMatch: (index: number, element: HTMLElement | null) => void
}) {
  if (matches.length === 0) return text

  const parts: React.ReactNode[] = []
  let cursor = 0
  matches.forEach((match) => {
    if (match.start > cursor) parts.push(text.slice(cursor, match.start))
    const active = match.index === activeMatchIndex
    parts.push(
      <mark
        key={match.start + "-" + match.index}
        ref={(element) => registerMatch(match.index, element)}
        className={active
          ? "rounded-[2px] bg-amber-300 px-0.5 text-black ring-2 ring-amber-500 ring-offset-1 ring-offset-background dark:bg-amber-300"
          : "rounded-[2px] bg-yellow-200 px-0.5 text-black dark:bg-yellow-400/80"}
      >
        {text.slice(match.start, match.end)}
      </mark>,
    )
    cursor = match.end
  })
  if (cursor < text.length) parts.push(text.slice(cursor))
  return parts
}

function humanSize(size: number): string {
  if (size < 1024) return `${size} B`
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`
  return `${(size / 1024 / 1024).toFixed(1)} MB`
}

function MetricCell({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="min-w-0 bg-background/95 px-3 py-2.5">
      <div className="truncate text-lg font-semibold leading-none tabular-nums">{value}</div>
      <div className="mt-1.5 truncate text-[11px] text-muted-foreground">{label}</div>
    </div>
  )
}

function ExtractionCell({
  label,
  extractedAt,
  formatTime,
  notExtracted,
}: {
  label: string
  extractedAt: string | null
  formatTime: (value: string | null) => string
  notExtracted: string
}) {
  return (
    <div className="min-w-0 bg-background/95 px-2.5 py-2.5">
      {extractedAt ? (
        <div className="flex min-w-0 items-center gap-1 text-xs font-medium text-emerald-600 dark:text-emerald-400">
          <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
          <span className="truncate tabular-nums">{formatTime(extractedAt)}</span>
        </div>
      ) : (
        <div className="truncate text-xs font-medium text-muted-foreground">{notExtracted}</div>
      )}
      <div className="mt-1.5 truncate text-[10px] text-muted-foreground">{label}</div>
    </div>
  )
}

export default function DocumentDetailPage() {
  const { locale, t } = useI18n()
  const { id, documentId } = useParams()
  const location = useLocation()
  const ksId = Number(id)
  const docId = Number(documentId)
  const [document, setDocument] = useState<DocumentMeta | null>(null)
  const [contribution, setContribution] = useState<DocumentContribution | null>(null)
  const [chunks, setChunks] = useState<Chunk[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState("")
  const [query, setQuery] = useState("")
  const [page, setPage] = useState(0)
  const [activeMatchIndex, setActiveMatchIndex] = useState(-1)
  const matchRefs = useRef(new Map<number, HTMLElement>())
  const sourceFolder = (location.state as { folder?: string } | null)?.folder
  const backPath = `/knowledge/${ksId}/documents${sourceFolder && sourceFolder !== "/" ? `?folder=${encodeURIComponent(sourceFolder)}` : ""}`

  useEffect(() => {
    if (!Number.isFinite(ksId) || !Number.isFinite(docId)) return
    let cancelled = false
    setLoading(true)
    setError("")
    Promise.all([
      api.getDocument(ksId, docId),
      api.getContribution(ksId, docId),
      api.getChunks(ksId, docId),
    ])
      .then(([nextDocument, nextContribution, nextChunks]) => {
        if (cancelled) return
        setDocument(nextDocument)
        setContribution(nextContribution)
        setChunks(nextChunks)
      })
      .catch((loadError) => {
        if (!cancelled) setError((loadError as Error).message)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => { cancelled = true }
  }, [docId, ksId])

  const filteredChunks = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    if (!normalized) return chunks
    return chunks.filter((chunk) =>
      chunk.text.toLowerCase().includes(normalized)
      || String(chunk.idx + 1).includes(normalized),
    )
  }, [chunks, query])

  const searchMatches = useMemo(() => {
    const searchTerm = query.trim()
    const byChunk = new Map<number, TextMatch[]>()
    const locations: TextMatch[] = []
    if (!searchTerm) return { byChunk, locations, total: 0 }

    filteredChunks.forEach((chunk, chunkPosition) => {
      const expression = new RegExp(escapeSearchTerm(searchTerm), "giu")
      const chunkMatches: TextMatch[] = []
      for (const match of chunk.text.matchAll(expression)) {
        if (match.index == null) continue
        const location = {
          index: locations.length,
          start: match.index,
          end: match.index + match[0].length,
          chunkPosition,
        }
        chunkMatches.push(location)
        locations.push(location)
      }
      if (chunkMatches.length > 0) byChunk.set(chunk.id, chunkMatches)
    })

    return { byChunk, locations, total: locations.length }
  }, [filteredChunks, query])

  useEffect(() => {
    setPage(0)
    setActiveMatchIndex(-1)
  }, [query])
  useEffect(() => {
    const lastPage = Math.max(0, Math.ceil(filteredChunks.length / CHUNK_PAGE_SIZE) - 1)
    setPage((current) => Math.min(current, lastPage))
  }, [filteredChunks.length])
  useEffect(() => {
    if (activeMatchIndex < 0) return
    const location = searchMatches.locations[activeMatchIndex]
    if (!location) return
    const targetPage = Math.floor(location.chunkPosition / CHUNK_PAGE_SIZE)
    if (page !== targetPage) {
      setPage(targetPage)
      return
    }
    const frame = window.requestAnimationFrame(() => {
      matchRefs.current.get(activeMatchIndex)?.scrollIntoView({
        behavior: "auto",
        block: "center",
      })
    })
    return () => window.cancelAnimationFrame(frame)
  }, [activeMatchIndex, page, searchMatches.locations])

  if (!Number.isFinite(ksId) || !Number.isFinite(docId)) {
    return <Navigate to="/" replace />
  }

  if (loading) {
    return (
      <div className="flex h-48 items-center justify-center text-sm text-muted-foreground">
        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        {t("common.loading")}
      </div>
    )
  }

  if (error || !document) {
    return (
      <div className="space-y-4">
        <Button asChild variant="ghost" size="sm">
          <Link to={backPath}><ArrowLeft className="h-4 w-4" />{t("documents.backToList")}</Link>
        </Button>
        <div className="rounded-lg border p-8 text-center text-sm text-muted-foreground">
          {t("documents.loadDetailFailed", { error: error || t("ontology.notFound") })}
        </div>
      </div>
    )
  }

  const formatTime = (value: string | null) => value
    ? new Date(value).toLocaleString(locale, { hour12: false })
    : "—"
  const parseLabel = document.parse_status === "parsed"
    ? t("documents.parsed")
    : document.parse_status === "failed" ? t("documents.parseFailed") : t("documents.notParsed")
  const shownChunks = filteredChunks.slice(page * CHUNK_PAGE_SIZE, (page + 1) * CHUNK_PAGE_SIZE)

  return (
    <div className="mx-auto w-full max-w-[1440px] space-y-6">
      <section className="overflow-hidden rounded-xl border bg-background shadow-sm">
        <div className="flex min-w-0 items-center gap-2.5 px-3 py-2.5">
          <Button asChild variant="ghost" size="icon-sm" className="shrink-0" title={t("documents.backToList")}>
            <Link to={backPath} aria-label={t("documents.backToList")}>
              <ArrowLeft className="h-4 w-4" />
            </Link>
          </Button>

          <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
          <div className="flex min-w-0 items-baseline gap-2">
            <h1 className="truncate text-base font-semibold">{document.original_filename}</h1>
            <span className="hidden shrink-0 text-xs text-muted-foreground sm:inline">{document.folder}</span>
          </div>

          <div className="ml-auto flex shrink-0 items-center gap-1.5">
            <Badge variant="outline" className="uppercase">{document.ext}</Badge>
            <Badge variant={document.parse_status === "failed" ? "destructive" : "secondary"}>{parseLabel}</Badge>
          </div>

          <dl className="hidden min-w-0 shrink-0 items-center gap-4 border-l pl-4 xl:flex">
            <div className="min-w-0">
              <dt className="text-[10px] text-muted-foreground">{t("documents.size")}</dt>
              <dd className="truncate text-xs font-medium tabular-nums">{humanSize(document.size_bytes)}</dd>
            </div>
            <div className="min-w-0">
              <dt className="text-[10px] text-muted-foreground">{t("documents.parser")}</dt>
              <dd className="max-w-36 truncate text-xs font-medium">{document.parser_backend || "—"}</dd>
            </div>
            <div className="min-w-0">
              <dt className="text-[10px] text-muted-foreground">{t("documents.uploadedAt")}</dt>
              <dd className="truncate text-xs font-medium tabular-nums">{formatTime(document.uploaded_at)}</dd>
            </div>
          </dl>
        </div>

        <dl className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t px-3 py-1.5 text-xs xl:hidden">
          <div className="flex gap-1.5"><dt className="text-muted-foreground">{t("documents.size")}</dt><dd>{humanSize(document.size_bytes)}</dd></div>
          <div className="flex min-w-0 gap-1.5"><dt className="text-muted-foreground">{t("documents.parser")}</dt><dd className="truncate">{document.parser_backend || "—"}</dd></div>
          <div className="flex gap-1.5"><dt className="text-muted-foreground">{t("documents.uploadedAt")}</dt><dd className="tabular-nums">{formatTime(document.uploaded_at)}</dd></div>
        </dl>

        <div className="grid grid-cols-2 gap-px border-t bg-border sm:grid-cols-3 lg:grid-cols-6">
          <MetricCell label={t("documents.chunks")} value={document.chunk_count.toLocaleString(locale)} />
          <MetricCell label={t("documents.characters")} value={document.text_char_count?.toLocaleString(locale) ?? "—"} />
          <MetricCell label={t("documents.tboxAxioms")} value={contribution?.axiom_count.toLocaleString(locale) ?? "—"} />
          <MetricCell label={t("documents.aboxIndividuals")} value={contribution?.individual_count.toLocaleString(locale) ?? "—"} />
          <ExtractionCell
            label={t("documents.schemaTbox")}
            extractedAt={document.tbox_extracted_at}
            formatTime={formatTime}
            notExtracted={t("documents.notExtracted")}
          />
          <ExtractionCell
            label={t("documents.instancesAbox")}
            extractedAt={document.abox_extracted_at}
            formatTime={formatTime}
            notExtracted={t("documents.notExtracted")}
          />
        </div>
      </section>

      <section className="mx-auto min-w-0 w-full max-w-5xl space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="text-sm font-semibold">
            {t("documents.chunks")} <span className="text-muted-foreground">({filteredChunks.length})</span>
          </h2>
          <div className="flex items-center gap-2">
            {searchMatches.total > 0 && (
              <span className="whitespace-nowrap text-xs text-muted-foreground tabular-nums" aria-live="polite">
                {t("documents.searchMatchPosition", {
                  current: activeMatchIndex >= 0 ? activeMatchIndex + 1 : 0,
                  total: searchMatches.total,
                })}
              </span>
            )}
            <div className="relative">
              <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key !== "Tab" || searchMatches.total === 0) return
                  event.preventDefault()
                  setActiveMatchIndex((current) => {
                    if (current < 0) return event.shiftKey ? searchMatches.total - 1 : 0
                    const direction = event.shiftKey ? -1 : 1
                    return (current + direction + searchMatches.total) % searchMatches.total
                  })
                }}
                placeholder={t("documents.searchChunks")}
                className="h-8 w-64 pl-8 text-sm"
              />
            </div>
          </div>
        </div>

        {chunks.length === 0 ? (
          <div className="rounded-lg border p-10 text-center text-sm text-muted-foreground">{t("documents.notParsedYet")}</div>
        ) : shownChunks.length === 0 ? (
          <div className="rounded-lg border p-10 text-center text-sm text-muted-foreground">{t("documents.noChunkMatches")}</div>
        ) : (
          <div className="space-y-3">
            {shownChunks.map((chunk) => (
              <article key={chunk.id} className="overflow-hidden rounded-lg border bg-card/20">
                <header className="flex flex-wrap items-center justify-between gap-2 border-b bg-muted/20 px-4 py-2.5 sm:px-5">
                  <Badge variant="secondary">{t("documents.chunkNumber", { number: chunk.idx + 1 })}</Badge>
                  <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
                    <span>{t("documents.chunkStats", { chars: chunk.text.length.toLocaleString(locale), tokens: chunk.token_estimate })}</span>
                    <span>{t("documents.characterRange", { start: chunk.char_start.toLocaleString(locale), end: chunk.char_end.toLocaleString(locale) })}</span>
                  </div>
                </header>
                <p className="whitespace-pre-wrap break-words px-4 py-4 text-sm leading-7 sm:px-6">
                  <HighlightedChunkText
                    text={chunk.text}
                    matches={searchMatches.byChunk.get(chunk.id) ?? []}
                    activeMatchIndex={activeMatchIndex}
                    registerMatch={(index, element) => {
                      if (element) matchRefs.current.set(index, element)
                      else matchRefs.current.delete(index)
                    }}
                  />
                </p>
              </article>
            ))}
          </div>
        )}

        <ReviewPagination
          page={page}
          pageSize={CHUNK_PAGE_SIZE}
          total={filteredChunks.length}
          onPageChange={(nextPage) => {
            setActiveMatchIndex(-1)
            setPage(nextPage)
          }}
        />
      </section>
    </div>
  )
}
