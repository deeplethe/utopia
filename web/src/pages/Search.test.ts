import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it, vi } from "vitest";
import { S } from "../i18n";
import { Search } from "./Search";

vi.mock("../kb", () => ({
  useKbId: () => "kb",
  useKb: () => ({ kb: { id: "kb" } }),
}));
vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => () => {},
  useSearch: () => ({ q: "orchard" }),
  Link: ({ children }: { children: React.ReactNode }) => children,
}));

function renderResults(hasMore: boolean) {
  const client = new QueryClient();
  client.setQueryData(["search", "kb", "orchard", 0], {
    results: Array.from({ length: 10 }, (_, seq) => ({
      id: `chunk-${seq}`, document_id: "doc", filename: "orchard.md",
      seq, text: `Search result ${seq + 1}`,
    })),
    has_more: hasMore,
  });
  return renderToStaticMarkup(
    createElement(QueryClientProvider, { client, children: createElement(Search) }),
  );
}

it("offers the next page only when the server says there are more hits", () => {
  const firstPage = renderResults(true);
  const onlyPage = renderResults(false);
  expect(firstPage).toContain("Search result 10");
  expect(onlyPage).toContain("Search result 10");
  expect(firstPage).toContain(S.search.nextPage);
  expect(onlyPage).not.toContain(S.search.nextPage);
});
