import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { ConceptMapping } from "../api";

import { S } from "../i18n";
import { en } from "../i18n/en";
import { zh } from "../i18n/zh";

vi.mock("../kb", () => ({ useKb: () => ({ kb: { id: "kb1" } }) }));
import { Mappings } from "./Mappings";

function render(mapping: ConceptMapping) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["mappings", "kb1", "all", "", 0], {
    items: [mapping], total: 1, counts: { proposed: 0, confirmed: 1, rejected: 0 },
  });
  return renderToStaticMarkup(createElement(QueryClientProvider, {
    client, children: createElement(Mappings),
  }));
}

const automatic: ConceptMapping = {
  id: "m1", concept_id: "e1", concept_name: "Revenue", source: "Warehouse",
  table_name: "orders", expr: "SUM(amount)", sql: null, unit: null,
  summary: null, derived: false, status: "confirmed", decided_by: null,
  confidence: 0.9, rationale: "The schema defines the amount column",
};

describe("mapping decision origin and evidence", () => {
  it("shows automatic origin and its evidence, then removes the badge after a human decision", () => {
    const html = render(automatic);
    expect(html).toContain(S.mapping.autoConfirmed);
    expect(html).toContain(S.mapping.reject);
    expect(html).toContain(S.mapping.approve);
    expect(html).toContain(S.mapping.confidence(0.9));
    expect(html).toContain(S.mapping.rationale(automatic.rationale!));
    expect(render({ ...automatic, decided_by: "editor" })).not.toContain(S.mapping.autoConfirmed);
    expect(render({ ...automatic, status: "proposed", confidence: null, rationale: null }))
      .not.toContain("90%");
    expect(en.mapping.confidence(0.8)).toBe("Confidence: 80%");
    expect(zh.mapping.confidence(0.8)).toBe("置信度：80%");
  });
});
