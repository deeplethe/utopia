import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api";
import { S } from "../i18n";
import { useKbId } from "../kb";
import { Button, Field, PageHeader, SearchSelect } from "../ui";
import { ExpressionDraftEditor } from "./ExpressionDraftEditor";
import { draftFromExpression, previewExpression, type ExpressionDraft } from "./expressionDraft";
import { expressionText, readExpression } from "./ruleExpressions";

/** Explicit unlisted route; a KB change remounts all local draft state. */
export function ExpressionDraftLab() {
  const kbId = useKbId();
  return <Lab key={kbId} kbId={kbId} />;
}

function Lab({ kbId }: { kbId: string }) {
  const ontology = useQuery({ queryKey: ["ontology", kbId], queryFn: () => api.ontology(kbId) });
  const rules = useQuery({ queryKey: ["rules", kbId], queryFn: () => api.rules(kbId) });
  const [draft, setDraft] = useState<ExpressionDraft>({ attr: "" });
  const [source, setSource] = useState("");
  const [unsupported, setUnsupported] = useState(false);
  if (ontology.isPending || rules.isPending) return <p role="status">{S.expressionDraft.loading}</p>;
  if (ontology.isError || rules.isError) return <div className="space-y-3">
    <p role="alert" className="text-small text-danger">{S.expressionDraft.loadError}</p>
    <Button onClick={() => { void ontology.refetch(); void rules.refetch(); }}>{S.expressionDraft.retry}</Button>
  </div>;
  const attributes = ontology.data.relation_types.filter((a) => a.kind === "attribute");
  // The ontology endpoint returns the full relation list; there is no client cap.
  const candidates = rules.data.rules.flatMap((rule) => [
    ...(rule.conclusion === "computed" ? [{ value: `${rule.id}/conclusion`, label: rule.name, hint: S.expressionDraft.conclusion, raw: rule.conclude_expr }] : []),
    ...rule.conditions.flatMap((c, i) => c.operand && typeof c.operand === "object" && !Array.isArray(c.operand)
      ? [{ value: `${rule.id}/${i}`, label: rule.name, hint: `${S.expressionDraft.condition} ${i + 1}`, raw: c.operand }] : []),
  ]);
  const preview = previewExpression(draft, new Set(attributes.map((a) => a.id)));
  return <div className="mx-auto w-full min-w-0 max-w-4xl space-y-6 p-6">
    <PageHeader title={S.expressionDraft.title} sub={S.expressionDraft.unsaved} />
    <p className="text-small text-ink-2">{S.expressionDraft.count(attributes.length)}</p>
    {!attributes.length && <p role="status" className="text-small text-ink-2">{S.expressionDraft.empty}</p>}
    <Field label={S.expressionDraft.existing}>
      <SearchSelect value={source} options={candidates} placeholder={S.expressionDraft.choose} onChange={(id) => {
        const raw = candidates.find((c) => c.value === id)?.raw;
        const expr = readExpression(raw);
        setSource(id); setUnsupported(!expr);
        if (expr) setDraft(draftFromExpression(expr));
      }} />
    </Field>
    {unsupported ? <p role="alert" className="text-small text-warn">{S.expressionDraft.unsupported}</p> : <>
      <ExpressionDraftEditor value={draft} onChange={setDraft} attributes={attributes} />
      <section className="space-y-2" aria-label={S.expressionDraft.preview}>
        <h2 className="text-title text-ink">{S.expressionDraft.preview}</h2>
        <p className="break-words text-body text-ink" data-preview="text">{preview ? expressionText(preview, attributes, S.ontology.ruleUnknownExpression) : S.expressionDraft.incomplete}</p>
        {preview && <pre className="whitespace-pre-wrap break-all text-small text-ink-2" data-preview="ast">{JSON.stringify(preview, null, 2)}</pre>}
      </section>
    </>}
    <Button onClick={() => { setDraft({ attr: "" }); setSource(""); setUnsupported(false); }}>{S.expressionDraft.reset}</Button>
  </div>;
}
