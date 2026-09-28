import { afterEach, describe, expect, it, vi } from "vitest";
import type { AlignmentItem, RelationTypeView } from "../api";
import { asksMarks, phraseStart } from "./alignmentPhrase";

type PhraseItem = Extract<AlignmentItem, { kind: "phrase" }>;

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

const item = (over: Partial<PhraseItem>): PhraseItem => ({
  kind: "phrase",
  id: "binding",
  phrase: "joined",
  subject_class: "person",
  object_class: "organization",
  object_is_value: false,
  statement_count: 2,
  examples: [],
  votes: null,
  decided_at: "2026-09-27T00:00:00Z",
  bound_to: null,
  direction: null,
  ...over,
});
const property = (key: string, temporal: string) => ({ key, temporal }) as RelationTypeView;

describe("what a single date marks", () => {
  it("is asked only under a state property", () => {
    const properties = [
      property("works_for", "state"),
      property("acquired", "event"),
      property("born_in", "eternal"),
    ];
    expect(asksMarks(properties, "works_for")).toBe(true);
    expect(asksMarks(properties, "acquired")).toBe(false);
    expect(asksMarks(properties, "born_in")).toBe(false);
    expect(asksMarks(properties, "")).toBe(false);
  });

  it("starts from the binding or the votes, and preselects no reading of a single date", () => {
    expect(
      phraseStart(
        item({
          votes: {
            first: { property: "works_for", direction: "forward", marks: "start" },
            second: { property: "works_for", direction: "forward", marks: "end" },
          },
        }),
      ),
    ).toEqual({ property: "works_for", direction: "forward", marks: null });
    expect(
      phraseStart(item({ votes: { first: null, second: { property: "leads", direction: "reverse" } } })),
    ).toEqual({ property: "leads", direction: "reverse", marks: null });
    // 绑上了、只差 marks 的：按现在的绑定预选属性与方向，读法仍留给人选
    expect(
      phraseStart(
        item({
          bound_to: "works_for",
          direction: "reverse",
          votes: {
            first: { property: "works_for", direction: "reverse", marks: "none" },
            second: { property: "works_for", direction: "reverse" },
          },
        }),
      ),
    ).toEqual({ property: "works_for", direction: "reverse", marks: null });
    expect(phraseStart(item({}))).toEqual({ property: "", direction: "forward", marks: null });
  });
});

for (const lang of ["en", "zh"]) {
  describe(`deciding a phrase (${lang})`, () => {
    it("sends what a single date marks with the decision", async () => {
      vi.stubGlobal("localStorage", { getItem: () => lang });
      const fetch = vi.fn(async () => Response.json({ ok: true, job_id: 1, status: "accepted" }, { status: 202 }));
      vi.stubGlobal("fetch", fetch);
      const { api } = await import("../api");
      await api.decideAlignmentPhrase("kb", "binding", "works_for", "forward", "start");
      await api.decideAlignmentPhrase("kb", "binding", "acquired", "forward", null);
      const bodies = fetch.mock.calls.map((call) => {
        const [path, init] = call as unknown as [string, RequestInit];
        expect(path).toBe("/api/v1/kbs/kb/review/alignment/phrases/binding");
        return JSON.parse(String(init.body));
      });
      expect(bodies).toEqual([
        { property: "works_for", direction: "forward", marks: "start" },
        { property: "acquired", direction: "forward", marks: null },
      ]);
    });

    it("words the refusals a single date can meet", async () => {
      vi.stubGlobal("localStorage", { getItem: () => lang });
      const { S } = await import("../i18n");
      const { api } = await import("../api");
      const worded = S.err as Record<string, string | undefined>;
      for (const code of ["empty_state_span", "unknown_marks", "marks_needs_state"]) {
        expect(worded[code], code).toMatch(/\S/);
        vi.stubGlobal(
          "fetch",
          vi.fn(async () => Response.json({ error: "server wording", code }, { status: 422 })),
        );
        await expect(
          api.decideAlignmentPhrase("kb", "binding", "works_for", "forward", "start"),
        ).rejects.toThrow(worded[code]);
      }
    });
  });
}
