import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { api } from "../api";
import { S } from "../i18n";
import { useKb, useKbId } from "../kb";
import { Pager, pageSlice } from "../ui";

const RESULT_PAGE = 10;

export function Search() {
  const kbId = useKbId();
  const { kb } = useKb();
  const [input, setInput] = useState("");
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(0);

  const results = useQuery({
    queryKey: ["search", kb?.id, query],
    queryFn: () => api.search(kb!.id, query),
    enabled: !!kb && query.length > 0,
  });

  return (
    <div className="h-full overflow-y-auto p-6">
      <div className="max-w-2xl mx-auto">
        <div className="flex gap-2 mb-6">
          <input
            className="input-dark flex-1 px-4 py-2.5 text-sm"
            placeholder={S.search.placeholder}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                setQuery(input.trim());
                setPage(0);
              }
            }}
          />
          <button
            onClick={() => {
              setQuery(input.trim());
              setPage(0);
            }}
            disabled={!input.trim()}
            className="u-btn u-btn-primary px-5 py-2 text-sm"
          >
            {S.search.button}
          </button>
        </div>

        {results.isFetching && <p className="text-sm text-neutral-500">{S.search.searching}</p>}
        {results.isError && (
          <p className="text-sm text-rose-400">{(results.error as Error).message}</p>
        )}
        {results.data && results.data.results.length === 0 && (
          <p className="text-sm text-neutral-500">{S.search.noResults}</p>
        )}

        <div className="space-y-3">
          {pageSlice(results.data?.results ?? [], page, RESULT_PAGE).rows.map((r) => (
            <Link
              key={r.id}
              to="/kb/$kbId/doc/$docId"
              params={{ kbId, docId: r.document_id }}
              search={{ chunk: r.id }}
              className="block glass rounded-xl p-4 glass-hover"
            >
              <div className="mb-1.5 text-xs text-neutral-500">
                {S.search.chunkOf(r.filename, r.seq + 1)}
              </div>
              <p className="text-sm text-neutral-300 leading-relaxed line-clamp-4 whitespace-pre-wrap">
                {r.text}
              </p>
            </Link>
          ))}
        </div>
        <Pager
          total={results.data?.results.length ?? 0}
          pageSize={RESULT_PAGE}
          page={page}
          onPage={setPage}
        />
      </div>
    </div>
  );
}
