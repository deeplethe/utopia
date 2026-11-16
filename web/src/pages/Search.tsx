import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { api } from "../api";
import { S } from "../i18n";
import { useKb, useKbId } from "../kb";
import { Button, Input, ROW_HOVER } from "../ui";
import { NextStep, nextStep, useReadiness } from "./NextStep";
import { fetchMe } from "../authSession";

const RESULT_PAGE = 10;

export function Search() {
  const kbId = useKbId();
  const { kb } = useKb();
  const navigate = useNavigate();
  /* 已提交的查询词以 URL 为唯一事实来源（同 review 的 queue、doc 的 chunk）：
     刷新、返回、分享链接都从地址栏重建，结果由 useQuery 自动重取 */
  const { q: query } = useSearch({ from: "/app/kb/$kbId/search" });
  // 打字是本地事，不打扰地址栏；前进/后退到别的 q 时输入框跟着走
  const [input, setInput] = useState(query ?? "");
  useEffect(() => setInput(query ?? ""), [query]);

  const me = useQuery({ queryKey: ["me"], queryFn: fetchMe });
  // 搜之前就该说清楚库里有没有东西可搜（#313）
  const readiness = useReadiness(kbId);
  const step = nextStep(readiness.data, {
    kbId,
    isAdmin: !!me.data?.is_admin,
    canUpload: kb?.my_role !== "viewer",
  });

  /* 提交 = 换地址，不是换 state：地址栏才是已提交查询词的唯一事实源。
     打字不写 URL，提交才写；空提交不动 */
  const submit = () => {
    const q = input.trim();
    if (!q) return;
    navigate({ to: "/kb/$kbId/search", params: { kbId }, search: { q } });
  };

  return (
    <div className="h-full overflow-y-auto p-6">
      <div className="max-w-2xl mx-auto">
        <div className="flex gap-2 mb-6">
          <Input className="flex-1"
            placeholder={S.search.placeholder}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing) submit();
            }}
          />
          <Button variant="primary" size="md"
            onClick={submit}
            disabled={!input.trim()}
          >
            {S.search.button}
          </Button>
        </div>

        {/* 一次都还没搜、而库里本来就没东西：与其等他敲一个词再回"没有结果"，
            不如现在就说下一步（#313） */}
        {!query && step && (
          <div className="py-8 grid place-items-center">
            <NextStep {...step} />
          </div>
        )}

        {kb?.id === kbId && query && (
          <SearchResults key={JSON.stringify([kbId, query])} kbId={kbId} query={query} />
        )}
      </div>
    </div>
  );
}

function SearchResults({ kbId, query }: { kbId: string; query: string }) {
  const [page, setPage] = useState(0);
  const offset = page * RESULT_PAGE;
  const results = useQuery({
    queryKey: ["search", kbId, query, offset],
    queryFn: ({ signal }) => api.search(kbId, query, offset, RESULT_PAGE, signal),
  });
  const hasMore = results.data?.has_more === true;

  return (
    <>
      {results.isFetching && <p className="text-body text-ink-2">{S.search.searching}</p>}
      {results.isError && !results.isFetching && (
        <div className="flex items-center gap-2">
          <p className="text-body text-danger">{results.error.message}</p>
          <Button variant="secondary" size="sm" onClick={() => void results.refetch()}>
            {S.search.retry}
          </Button>
        </div>
      )}
      {results.data && results.data.results.length === 0 && (
        <p className="text-body text-ink-2">{S.search.noResults}</p>
      )}

      {/* 一个面板装多行（DESIGN.md 6）：悬停归行，槽不响应指针 */}
      <div className="glass rounded-panel divide-y divide-line">
        {(results.data?.results ?? []).map((r) => (
          <Link
            key={r.id}
            to="/kb/$kbId/doc/$docId"
            params={{ kbId, docId: r.document_id }}
            search={{ chunk: r.id }}
            className={`block p-4 ${ROW_HOVER}`}
          >
            <div className="mb-2 text-small text-ink-2">
              {S.search.chunkOf(r.filename, r.seq + 1)}
            </div>
            <p className="text-body text-ink-2 leading-relaxed line-clamp-4 whitespace-pre-wrap">
              {r.text}
            </p>
          </Link>
        ))}
      </div>
      {(page > 0 || hasMore) && (
        <div className="mt-3 flex justify-end gap-2">
          <Button
            variant="secondary"
            size="sm"
            disabled={page === 0}
            onClick={() => setPage(page - 1)}
          >
            {S.search.previousPage}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            disabled={!hasMore || results.isFetching || results.isError}
            onClick={() => setPage(page + 1)}
          >
            {S.search.nextPage}
          </Button>
        </div>
      )}
    </>
  );
}
