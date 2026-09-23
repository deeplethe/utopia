import { useMemo } from "react";
import type { RelationTypeView } from "../api";
import { S } from "../i18n";
import { Dropdown, Field, Input, SearchSelect, type SearchSelectOption } from "../ui";
import type { ExpressionDraft } from "./expressionDraft";

/** Controlled tree editor: replacing a node never rewrites its siblings or grouping. */
export function ExpressionDraftEditor({ value, onChange, attributes }: {
  value: ExpressionDraft;
  onChange: (value: ExpressionDraft) => void;
  attributes: RelationTypeView[];
}) {
  const options = useMemo(() => attributes.map((a) => ({
    value: a.id, label: a.label,
    hint: `${a.key} · ${a.datatype ?? S.expressionDraft.undeclared} · ${a.unit ?? S.expressionDraft.undeclared} · ${a.id}`,
  })), [attributes]);
  return <DraftNode value={value} onChange={onChange} options={options} depth={0} path="root" />;
}

function DraftNode({ value, onChange, options, depth, path }: {
  value: ExpressionDraft; onChange: (value: ExpressionDraft) => void;
  options: SearchSelectOption[]; depth: number; path: string;
}) {
  const selected = "attr" in value ? options.find((a) => a.value === value.attr) : undefined;
  const kind = "attr" in value ? "attr" : "const" in value ? "const" : value.op;
  const kinds = [
    { value: "attr", label: S.expressionDraft.attribute },
    { value: "const", label: S.expressionDraft.constant },
    ...(depth < 4 ? ["add", "sub", "mul", "div"].map((op) => ({ value: op, label: S.expressionDraft[op as "add" | "sub" | "mul" | "div"] })) : []),
  ];
  const changeKind = (next: string) => {
    if (next === kind) return;
    if (next === "attr") onChange({ attr: "" });
    else if (next === "const") onChange({ const: "" });
    else onChange({ op: next as "add" | "sub" | "mul" | "div", l: "op" in value ? value.l : value, r: "op" in value ? value.r : { attr: "" } });
  };
  return <fieldset className="min-w-0 space-y-2 border-l border-line pl-2" data-node={path}>
    <legend className="text-small text-ink-2">{depth === 0 ? S.expressionDraft.expression : path.endsWith("l") ? S.expressionDraft.left : S.expressionDraft.right}</legend>
    <Dropdown value={kind} options={kinds} onChange={changeKind} menuLabel={S.expressionDraft.kind} className="w-full" />
    {depth === 4 && <p className="text-fine text-ink-2">{S.expressionDraft.depthLimit}</p>}
    {"attr" in value ? <Field label={S.expressionDraft.attribute}>
      <SearchSelect value={value.attr} options={options} onChange={(attr) => onChange({ attr })} placeholder={S.expressionDraft.choose} className="w-full min-w-0" />
      {selected && <p className="break-words text-small text-ink-2">{selected.label} · {selected.hint}</p>}
      {value.attr && !selected && <p role="alert" className="break-all text-small text-warn">{S.expressionDraft.missing}: {value.attr}</p>}
    </Field> : "const" in value ? <Field label={S.expressionDraft.constant}>
      <Input aria-label={S.expressionDraft.constant} value={value.const} onChange={(e) => onChange({ const: e.target.value })} />
    </Field> : <div className="space-y-3">
      <DraftNode value={value.l} onChange={(l) => onChange({ ...value, l })} options={options} depth={depth + 1} path={`${path}.l`} />
      <DraftNode value={value.r} onChange={(r) => onChange({ ...value, r })} options={options} depth={depth + 1} path={`${path}.r`} />
    </div>}
  </fieldset>;
}
