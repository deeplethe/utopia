import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { api, type OntologyRegressionCase } from "../api";
import { S } from "../i18n";
import { OntologyRegressionsPanel } from "./OntologyRegressionsPanel";

const base: OntologyRegressionCase = {
  id: "case", kb_id: "kb", statement_id: "statement",
  expected_property_id: "works-for", expected_direction: "forward",
  created_by: "actor", created_at: "2026-10-10T00:00:00Z", origin: "adoption",
  last_checked_at: "2026-10-10T01:00:00Z",
  last_result: {
    passed: true, human_bound: false, actual_property_id: "works-for",
    actual_direction: "forward", status: "bound", decided_at: "2026-10-10T01:00:00Z",
  },
  subject_id: "person", subject_label: "Alice", phrase: "works at",
  object_id: "company", object_label: "Acme", object_value: null,
  expected_property_label: "Works for", expected_property_key: "works_for",
  actual_property_label: "Works for", actual_property_key: "works_for",
  created_by_label: "Reviewer",
};

function render(cases: OntologyRegressionCase[]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["ontologyRegressions", "kb"], { cases });
  return renderToStaticMarkup(createElement(QueryClientProvider, {
    client,
    children: createElement(OntologyRegressionsPanel, { kbId: "kb" }),
  }));
}

describe("stored property regression comparisons", () => {
  it("distinguishes mismatches, matches and missing checks without requesting evidence", () => {
    const evidence = vi.spyOn(api, "factEvidence");
    try {
      const html = render([
        base,
        { ...base, id: "wrong-direction", last_result: { ...base.last_result!, passed: false, actual_direction: "reverse" } },
        { ...base, id: "pending", last_result: null, last_checked_at: null, actual_property_label: null },
      ]);
      for (const text of [S.ontology.casePassed, S.ontology.caseMismatch, S.ontology.caseNotChecked, S.ontology.caseForward, S.ontology.caseReverse, "Alice", "Acme", "Reviewer"]) {
        expect(html).toContain(text);
      }
      expect(evidence).not.toHaveBeenCalled();
    } finally {
      evidence.mockRestore();
    }
  });

  it("keeps a recorded binding distinct from an undecided result when its property is unavailable", () => {
    const html = render([{ ...base, actual_property_label: null, actual_property_key: null }]);
    expect(html).toContain(S.ontology.caseUnavailableProperty);
    expect(html).toContain(S.ontology.caseDecidedAt);
    expect(html).not.toContain(S.ontology.caseUndecided);
  });

  it("calls a preserved manual binding a human decision instead of an independent match", () => {
    const html = render([{ ...base, last_result: { ...base.last_result!, human_bound: true } }]);
    expect(html).toContain(S.ontology.caseHumanBound);
    expect(html).toContain(S.ontology.caseHumanHint);
    expect(html).not.toContain(`>${S.ontology.casePassed}<`);
  });
});
