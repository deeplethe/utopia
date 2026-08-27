import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { api, type ChunkFact } from "../api";
import { S } from "../i18n";
import { Pager, pageSlice } from "../ui";
import { SourcesRail } from "./SourcesRail";

const DOC_PAGE = 12;

const ym = (iso: string | null) => (iso ? iso.slice(0, 7) : null);

function factRange(f: ChunkFact): string | null {
  if (!f.valid_from && !f.valid_to) return null;
  return `${ym(f.valid_from) ?? "…"} → ${ym(f.valid_to) ?? S.doc.ongoing}`;
}

export function DocViewer() {
  const { docId } = useParams({ from: "/app/doc/$docId" });
  const { chunk } = useSearch({ from: "/app/doc/$docId" });
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
    return <div className="p-8 text-sm text-neutral-500">{S.doc.loading}</div>;
  if (detail.isError)
    return (
      <div className="p-8 text-sm text-rose-400">
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
        onSelect={(sel) => navigate({ to: "/library", search: { src: sel } })}
      />
      <div className="flex-1 min-w-0 overflow-y-auto u-scroll">
        <div className="max-w-4xl mx-auto p-6">
        <div className="mb-4 flex items-baseline justify-between gap-4">
          <div>
            <h2 className="text-lg font-bold text-neutral-100 break-all">{doc.filename}</h2>
            <p className="mt-0.5 text-xs text-neutral-500">
              {chunks.length} {S.doc.sections} · {(doc.size_bytes / 1024).toFixed(0)} KB ·{" "}
              {new Date(doc.created_at).toLocaleDateString()}
            </p>
          </div>
          <Link to="/library" className="shrink-0 text-sm text-[var(--u-accent)] hover:underline">
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
                  className={`flex-1 min-w-0 rounded-xl border p-4 text-sm leading-relaxed whitespace-pre-wrap border-white/10 transition-[border-color,background-color,box-shadow] duration-700 ${
                    hit && flash ? "u-flash" : "bg-white/[0.04]"
                  }`}
                >
                  <div className="mb-1.5 text-xs text-neutral-500">
                    {S.doc.section} {c.seq + 1}
                    {hit && (
                      <span
                        className={`ml-2 text-[var(--u-accent)] transition-opacity duration-700 ${
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
                  <aside className="w-64 shrink-0 rounded-xl border border-white/10 bg-white/[0.02] p-3">
                    <div className="mb-2 text-[10px] font-medium uppercase tracking-[0.08em] text-neutral-600">
                      {S.doc.extracted} · {facts.length}
                    </div>
                    <div className="space-y-2">
                      {facts.map((f) => {
                        const range = factRange(f);
                        return (
                          <div key={f.fact_id} className="text-xs leading-snug">
                            <div>
                              <Link
                                to="/graph"
                                search={{ entity: f.subject_id }}
                                className="text-neutral-200 hover:text-white hover:underline underline-offset-2 decoration-white/30"
                              >
                                {f.subject}
                              </Link>
                              <span className="text-neutral-600"> {f.predicate} </span>
                              {f.object_id ? (
                                <Link
                                  to="/graph"
                                  search={{ entity: f.object_id }}
                                  className="text-neutral-200 hover:text-white hover:underline underline-offset-2 decoration-white/30"
                                >
                                  {f.object}
                                </Link>
                              ) : (
                                <span className="text-neutral-300">{f.object ?? ""}</span>
                              )}
                            </div>
                            {range && <div className="u-num text-[10.5px] text-neutral-500">{range}</div>}
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
