import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { api, type ChunkFact } from "../api";
import { S } from "../i18n";
import { useKbId } from "../kb";
import { Pager, pageSlice } from "../ui";
import { SourcesRail } from "./SourcesRail";

const DOC_PAGE = 12;

const ym = (iso: string | null) => (iso ? iso.slice(0, 7) : null);

function factRange(f: ChunkFact): string | null {
  if (!f.valid_from && !f.valid_to) return null;
  return `${ym(f.valid_from) ?? "…"} → ${ym(f.valid_to) ?? S.doc.ongoing}`;
}

export function DocViewer() {
  const kbId = useKbId();
  const { docId } = useParams({ from: "/app/kb/$kbId/doc/$docId" });
  const { chunk } = useSearch({ from: "/app/kb/$kbId/doc/$docId" });
  const navigate = useNavigate();

  const detail = useQuery({
    queryKey: ["docDetail", docId],
    queryFn: () => api.documentDetail(docId),
  });
  // 反向证据链：各分块抽出的事实（一次取整文档，按 chunk 分组）
  const extractions = useQuery({
    queryKey: ["docExtractions", docId],
    queryFn: () => api.documentExtractions(docId),
  });
  const factsByChunk = useMemo(() => {
    const map = new Map<string, ChunkFact[]>();
    for (const f of extractions.data?.facts ?? []) {
      if (!map.has(f.chunk_id)) map.set(f.chunk_id, []);
      map.get(f.chunk_id)!.push(f);
    }
    return map;
  }, [extractions.data]);

  // null = 自动定位：有引用跳转时翻到目标分块所在页
  const [page, setPage] = useState<number | null>(null);
  useEffect(() => setPage(null), [chunk, docId]);

  const highlightRef = useRef<HTMLDivElement>(null);
  // 引用跳转：滚动到目标分块点亮一下，稍候淡出恢复普通状态（不常驻高亮）
  const [flash, setFlash] = useState(false);
  useEffect(() => {
    if (detail.data && highlightRef.current) {
      highlightRef.current.scrollIntoView({ behavior: "smooth", block: "center" });
      setFlash(true);
      // 平滑滚动本身耗几百毫秒，点亮窗口要留足滚动后的可视时间
      const t = setTimeout(() => setFlash(false), 2600);
      return () => clearTimeout(t);
    }
  }, [detail.data, chunk]);

  if (detail.isPending)
    return <div className="p-8 text-body text-ink-3">{S.doc.loading}</div>;
  if (detail.isError)
    return (
      <div className="p-8 text-body text-danger">
        {(detail.error as Error).message}
      </div>
    );

  const { document: doc, chunks } = detail.data;
  const targetIdx = chunk ? chunks.findIndex((c) => c.id === chunk) : -1;
  const curPage = page ?? (targetIdx >= 0 ? Math.floor(targetIdx / DOC_PAGE) : 0);
  const { rows: pagedChunks, safe: safePage } = pageSlice(chunks, curPage, DOC_PAGE);

  return (
    <div className="h-full flex">
      {/* 来源栏常驻：当前文档所属文件夹高亮，点其他文件夹跳回 Library 对应视图 */}
      <SourcesRail
        kbId={doc.kb_id}
        active={doc.source_id ?? "uploads"}
        onSelect={(sel) => navigate({ to: "/kb/$kbId/library", params: { kbId }, search: { src: sel } })}
      />
      <div className="flex-1 min-w-0 overflow-y-auto u-scroll">
        <div className="max-w-4xl mx-auto p-6">
        <div className="mb-4 flex items-baseline justify-between gap-4">
          <div>
            <h2 className="text-title font-semibold text-ink break-all">{doc.filename}</h2>
            <p className="mt-1 text-small text-ink-3">
              {chunks.length} {S.doc.sections} · {(doc.size_bytes / 1024).toFixed(0)} KB ·{" "}
              {new Date(doc.created_at).toLocaleDateString()}
            </p>
          </div>
          <Link to="/kb/$kbId/library" params={{ kbId }} className="u-link shrink-0 text-body">
            {S.doc.backToLibrary}
          </Link>
        </div>

        <div className="space-y-3">
          {pagedChunks.map((c) => {
            const hit = c.id === chunk;
            const facts = factsByChunk.get(c.id) ?? [];
            return (
              <div key={c.id} ref={hit ? highlightRef : undefined} className="flex gap-3">
                <div
                  className={`u-chunk flex-1 min-w-0 rounded-xl border p-4 text-body leading-relaxed whitespace-pre-wrap border-line ${
                    hit && flash ? "u-flash" : "bg-surface"
                  }`}
                >
                  <div className="mb-2 text-small text-ink-3">
                    {S.doc.section} {c.seq + 1}
                    {hit && (
                      <span
                        className={`u-fade-slow ml-2 text-accent ${
                          flash ? "opacity-100" : "opacity-0"
                        }`}
                      >
                        {S.doc.citedHere}
                      </span>
                    )}
                  </div>
                  {c.text}
                </div>

                {/* 抽取对照栏：这个分块产出了哪些事实（实体可跳图谱） */}
                {facts.length > 0 && (
                  <aside className="w-64 shrink-0 rounded-xl border border-line bg-surface p-3">
                    <div className="mb-2 text-fine font-medium uppercase tracking-[0.08em] text-ink-3">
                      {S.doc.extracted} · {facts.length}
                    </div>
                    <div className="space-y-2">
                      {facts.map((f) => {
                        const range = factRange(f);
                        return (
                          <div key={f.fact_id} className="text-small leading-snug">
                            <div>
                              <Link
                                to="/kb/$kbId/graph"
                                params={{ kbId }}
                                search={{ entity: f.subject_id }}
                                className="u-inline-link text-ink"
                              >
                                {f.subject}
                              </Link>
                              <span
                                className={
                                  f.predicate === null
                                    ? "italic text-ink-3"
                                    : "text-ink-3"
                                }
                                title={
                                  f.predicate && f.inferred
                                    ? S.graph.inferredPredicate
                                    : undefined
                                }
                              >
                                {" "}
                                {f.predicate ?? S.graph.unknownPredicate}{" "}
                              </span>
                              {f.object_id ? (
                                <Link
                                  to="/kb/$kbId/graph"
                                  params={{ kbId }}
                                  search={{ entity: f.object_id }}
                                  className="u-inline-link text-ink"
                                >
                                  {f.object}
                                </Link>
                              ) : (
                                <span className="text-ink-2">{f.object ?? ""}</span>
                              )}
                            </div>
                            {range && <div className="u-num text-fine text-ink-3">{range}</div>}
                          </div>
                        );
                      })}
                    </div>
                  </aside>
                )}
              </div>
            );
          })}
        </div>
          <Pager total={chunks.length} pageSize={DOC_PAGE} page={safePage} onPage={setPage} />
        </div>
      </div>
    </div>
  );
}
