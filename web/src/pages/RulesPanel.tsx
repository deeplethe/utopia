/** 业务规则的编写台（0021 / #277）。
 *
 *  **规则读起来要像一句话**：「Well 及其子类，当 全烃 大于 8 且 解释 属于 {…}，
 *  得出 GasBearingWell」。所以这里不做表达式输入框——结构化的下拉与数字框既是
 *  录入方式，也是它唯一的显示方式，两者不会漂移。
 *
 *  条件整组提交而不是逐条打补丁：一个合取是一个整体，没有「改到一半」的状态。
 *
 *  样式按 web/DESIGN.md 那五条：字号五档、间距六档、颜色只用 token、控件与状态
 *  一律从 ui/ 来。 */
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Play, Plus, Trash2 } from "lucide-react";
import {
  api,
  type BusinessRule,
  type EntityTypeView,
  type RelationTypeView,
  type RuleCondition,
} from "../api";
import { S } from "../i18n";
import {
  Button,
  DangerConfirm,
  Dropdown,
  IconButton,
  Input,
  Panel,
} from "../ui";
import { toast } from "../toast";

/** op → 那句话里的动词。**数字与集合两类分开**，因为它们的操作数长得不一样 */
const OPS: {
  value: string;
  label: () => string;
  operand: "num" | "range" | "set" | "none";
}[] = [
  { value: "gt", label: () => S.ontology.ruleOpGt, operand: "num" },
  { value: "gte", label: () => S.ontology.ruleOpGte, operand: "num" },
  { value: "lt", label: () => S.ontology.ruleOpLt, operand: "num" },
  { value: "lte", label: () => S.ontology.ruleOpLte, operand: "num" },
  { value: "between", label: () => S.ontology.ruleOpBetween, operand: "range" },
  { value: "in", label: () => S.ontology.ruleOpIn, operand: "set" },
  { value: "present", label: () => S.ontology.ruleOpPresent, operand: "none" },
];

const operandKind = (op: string) =>
  OPS.find((o) => o.value === op)?.operand ?? "num";

/** 条件的操作数 → 输入框里的文本。回读要与写入是同一套，否则编辑一次就变形 */
function operandText(op: string, operand: unknown): string {
  const kind = operandKind(op);
  if (kind === "none") return "";
  if (kind === "set") return Array.isArray(operand) ? operand.join(", ") : "";
  if (kind === "range") return Array.isArray(operand) ? operand.join(" - ") : "";
  return operand === null || operand === undefined ? "" : String(operand);
}

/** 输入框文本 → 操作数。**解析不出来就返回 undefined**，由调用方拦在保存之前 */
function parseOperand(op: string, text: string): unknown | undefined {
  const kind = operandKind(op);
  if (kind === "none") return undefined;
  const t = text.trim();
  if (kind === "set") {
    const set = t
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    return set.length ? set : undefined;
  }
  if (kind === "range") {
    const parts = t.split(/[-~]/).map((s) => Number(s.trim()));
    return parts.length === 2 && parts.every((n) => Number.isFinite(n))
      ? parts
      : undefined;
  }
  const n = Number(t);
  return Number.isFinite(n) ? n : undefined;
}

type Draft = {
  name: string;
  description: string;
  subject_type_id: string;
  conclusion: "typing" | "attribute";
  conclude_type_id: string;
  conclude_predicate_id: string;
  conclude_value: string;
  conditions: { predicate_id: string; op: string; text: string }[];
};

const emptyDraft = (
  classes: EntityTypeView[],
  attrs: RelationTypeView[],
): Draft => ({
  name: "",
  description: "",
  subject_type_id: classes[0]?.id ?? "",
  conclusion: "typing",
  conclude_type_id: classes[0]?.id ?? "",
  conclude_predicate_id: attrs[0]?.id ?? "",
  conclude_value: "",
  conditions: attrs[0]
    ? [{ predicate_id: attrs[0].id, op: "gt", text: "" }]
    : [],
});

export function RulesPanel({
  kbId,
  classes,
  attributes,
  onError,
}: {
  kbId: string;
  classes: EntityTypeView[];
  /** kind='attribute' 的谓词——规则只读实体自己的字面值 */
  attributes: RelationTypeView[];
  onError: (e: unknown) => void;
}) {
  const qc = useQueryClient();
  const rules = useQuery({
    queryKey: ["rules", kbId],
    queryFn: () => api.rules(kbId),
  });
  const [draft, setDraft] = useState<Draft | null>(null);
  /** 待确认删除的那一条。删规则会带走它推出的全部结论，值得停一下 */
  const [doomed, setDoomed] = useState<BusinessRule | null>(null);

  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ["rules", kbId] });
    qc.invalidateQueries({ queryKey: ["graph"] });
    qc.invalidateQueries({ queryKey: ["entity", kbId] });
  };

  const save = useMutation({
    mutationFn: () => {
      const d = draft!;
      if (!d.conditions.length) throw new Error(S.ontology.ruleNeedsCondition);
      const conditions: RuleCondition[] = d.conditions.map((c) => {
        const operand = parseOperand(c.op, c.text);
        if (operandKind(c.op) !== "none" && operand === undefined) {
          throw new Error(S.ontology.ruleNeedsCondition);
        }
        return { predicate_id: c.predicate_id, op: c.op, operand };
      });
      return api.createRule(kbId, {
        name: d.name,
        description: d.description,
        subject_type_id: d.subject_type_id,
        conclusion: d.conclusion,
        conclude_type_id:
          d.conclusion === "typing" ? d.conclude_type_id : undefined,
        conclude_predicate_id:
          d.conclusion === "attribute" ? d.conclude_predicate_id : undefined,
        conclude_value:
          d.conclusion === "attribute" ? d.conclude_value : undefined,
        conditions,
      });
    },
    onSuccess: () => {
      toast.success(S.ontology.ruleSaved);
      setDraft(null);
      invalidate();
    },
    onError: (e: Error) => toast.error(e.message),
  });

  const run = useMutation({
    mutationFn: () => api.runRules(kbId),
    onSuccess: (r) => {
      toast.success(S.ontology.ruleRunDone(r.hits, r.inserted, r.invalidated));
      // 展开不全要单独说：少推几条与「不满足」在结果里长得一样
      if (r.capped) toast.error(S.ontology.ruleRunCapped(r.capped));
      invalidate();
    },
    onError,
  });

  const toggle = useMutation({
    mutationFn: (r: BusinessRule) =>
      api.updateRule(kbId, r.id, { enabled: !r.enabled }),
    onSuccess: invalidate,
    onError,
  });

  const remove = useMutation({
    mutationFn: (id: string) => api.deleteRule(kbId, id),
    onSuccess: () => {
      toast.success(S.ontology.ruleDeleted);
      invalidate();
    },
    onError,
  });

  const list = rules.data?.rules ?? [];

  return (
    <div className="space-y-6">
      {doomed && (
        <DangerConfirm
          title={S.ontology.ruleDelete}
          hint={S.ontology.ruleDeleteConfirm(doomed.name)}
          confirmLabel={S.ontology.ruleDelete}
          cancelLabel={S.graph.editCancel}
          busy={remove.isPending}
          onConfirm={() => {
            remove.mutate(doomed.id);
            setDoomed(null);
          }}
          onCancel={() => setDoomed(null)}
        />
      )}

      <div className="space-y-2">
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="text-title font-medium text-ink">
            {S.ontology.rulesTitle}
          </h2>
          {list.length > 0 && (
            <span className="u-num text-small text-ink-3">{list.length}</span>
          )}
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto"
            onClick={() => run.mutate()}
            disabled={run.isPending || !list.length}
          >
            <Play size={12} />
            {run.isPending ? S.ontology.ruleRunning : S.ontology.ruleRun}
          </Button>
        </div>
        <p className="text-fine leading-relaxed text-ink-3">
          {S.ontology.rulesHint}
        </p>
      </div>

      {list.map((r) => (
        <Panel key={r.id} className="space-y-2 p-4">
          <div className="flex items-center gap-2">
            <span className="text-body font-medium text-ink">{r.name}</span>
            {/* 此刻凭它成立的结论条数——规则有没有在干活，一眼看得见 */}
            {r.derived_count > 0 && (
              <span className="u-num text-fine text-ink-3">
                {S.ontology.ruleDerivedCount(r.derived_count)}
              </span>
            )}
            <div className="ml-auto flex items-center gap-2">
              {/* 开关是个动作，所以是 Button；启用与否用 variant 区分，
                  而不是拿 Chip 当按钮——Chip 是状态标签，不接受点击 */}
              <Button
                size="sm"
                variant={r.enabled ? "secondary" : "ghost"}
                onClick={() => toggle.mutate(r)}
                aria-pressed={r.enabled}
              >
                {r.enabled ? S.ontology.ruleEnabled : S.ontology.ruleDisabled}
              </Button>
              <IconButton
                label={S.ontology.ruleDelete}
                size="sm"
                onClick={() => setDoomed(r)}
              >
                <Trash2 size={12} />
              </IconButton>
            </div>
          </div>
          {r.description && (
            <p className="text-small leading-relaxed text-ink-3">
              {r.description}
            </p>
          )}
          {/* 规则读成一句话。这一段就是它的全部语义，没有别处再藏着条件 */}
          <p className="text-small leading-relaxed text-ink-2">
            <span className="text-ink-3">{S.ontology.ruleSubject} </span>
            {r.subject_label}
            <span className="text-ink-3"> ({S.ontology.ruleSubjectHint})</span>
            <span className="text-ink-3">, {S.ontology.ruleConditions} </span>
            {r.conditions.map((c, i) => (
              <span key={i}>
                {i > 0 && <span className="text-ink-3"> · </span>}
                <span className="text-ink">{c.predicate_label}</span>{" "}
                <span className="text-ink-3">
                  {OPS.find((o) => o.value === c.op)?.label() ?? c.op}
                </span>{" "}
                <span className="u-num text-ink">
                  {operandText(c.op, c.operand)}
                </span>
              </span>
            ))}
            <span className="text-ink-3"> → {S.ontology.ruleConcludes} </span>
            <span className="text-ink">
              {r.conclusion === "typing"
                ? r.conclude_type_label
                : `${r.conclude_predicate_label} = ${JSON.stringify(r.conclude_value)}`}
            </span>
          </p>
        </Panel>
      ))}

      {!list.length && !draft && (
        <p className="text-small text-ink-3">{S.ontology.rulesEmpty}</p>
      )}

      {draft ? (
        <Panel className="space-y-3 p-4">
          <Input
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
            placeholder={S.ontology.ruleNamePlaceholder}
            className="w-full"
          />
          <Input
            value={draft.description}
            onChange={(e) => setDraft({ ...draft, description: e.target.value })}
            placeholder={S.ontology.ruleDescription}
            className="w-full"
          />
          <div className="flex items-center gap-2">
            <span className="shrink-0 text-fine text-ink-3">
              {S.ontology.ruleSubject}
            </span>
            <Dropdown
              value={draft.subject_type_id}
              onChange={(v) => setDraft({ ...draft, subject_type_id: v })}
              options={classes.map((c) => ({ value: c.id, label: c.label }))}
            />
          </div>

          <div className="space-y-2">
            <div className="text-fine text-ink-3">
              {S.ontology.ruleConditions}
            </div>
            {draft.conditions.map((c, i) => (
              <div key={i} className="flex items-center gap-2">
                <Dropdown
                  value={c.predicate_id}
                  onChange={(v) => {
                    const next = [...draft.conditions];
                    next[i] = { ...c, predicate_id: v };
                    setDraft({ ...draft, conditions: next });
                  }}
                  options={attributes.map((a) => ({
                    value: a.id,
                    label: a.label,
                  }))}
                />
                <Dropdown
                  value={c.op}
                  onChange={(v) => {
                    const next = [...draft.conditions];
                    next[i] = { ...c, op: v, text: "" };
                    setDraft({ ...draft, conditions: next });
                  }}
                  options={OPS.map((o) => ({
                    value: o.value,
                    label: o.label(),
                  }))}
                />
                {operandKind(c.op) !== "none" && (
                  <Input
                    value={c.text}
                    onChange={(e) => {
                      const next = [...draft.conditions];
                      next[i] = { ...c, text: e.target.value };
                      setDraft({ ...draft, conditions: next });
                    }}
                    placeholder={
                      operandKind(c.op) === "set"
                        ? S.ontology.ruleOperandSet
                        : S.ontology.ruleOperandNumber
                    }
                    className="u-num flex-1"
                  />
                )}
                <IconButton
                  label={S.ontology.ruleDelete}
                  size="sm"
                  onClick={() =>
                    setDraft({
                      ...draft,
                      conditions: draft.conditions.filter((_, j) => j !== i),
                    })
                  }
                >
                  <Trash2 size={11} />
                </IconButton>
              </div>
            ))}
            <Button
              size="sm"
              variant="ghost"
              onClick={() =>
                setDraft({
                  ...draft,
                  conditions: [
                    ...draft.conditions,
                    { predicate_id: attributes[0]?.id ?? "", op: "gt", text: "" },
                  ],
                })
              }
            >
              <Plus size={11} />
              {S.ontology.ruleAddCondition}
            </Button>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <span className="shrink-0 text-fine text-ink-3">
              {S.ontology.ruleConcludes}
            </span>
            <Dropdown
              value={draft.conclusion}
              onChange={(v) =>
                setDraft({ ...draft, conclusion: v as "typing" | "attribute" })
              }
              options={[
                { value: "typing", label: S.ontology.ruleConcludesTyping },
                { value: "attribute", label: S.ontology.ruleConcludesAttribute },
              ]}
            />
            {draft.conclusion === "typing" ? (
              <Dropdown
                value={draft.conclude_type_id}
                onChange={(v) => setDraft({ ...draft, conclude_type_id: v })}
                options={classes.map((c) => ({ value: c.id, label: c.label }))}
              />
            ) : (
              <>
                <Dropdown
                  value={draft.conclude_predicate_id}
                  onChange={(v) =>
                    setDraft({ ...draft, conclude_predicate_id: v })
                  }
                  options={attributes.map((a) => ({
                    value: a.id,
                    label: a.label,
                  }))}
                />
                <Input
                  value={draft.conclude_value}
                  onChange={(e) =>
                    setDraft({ ...draft, conclude_value: e.target.value })
                  }
                  className="flex-1"
                />
              </>
            )}
          </div>

          <div className="flex justify-end gap-2">
            <Button size="sm" variant="ghost" onClick={() => setDraft(null)}>
              {S.graph.editCancel}
            </Button>
            <Button size="sm" onClick={() => save.mutate()} disabled={save.isPending}>
              {S.ontology.ruleSave}
            </Button>
          </div>
        </Panel>
      ) : (
        <Button
          size="sm"
          variant="ghost"
          onClick={() => setDraft(emptyDraft(classes, attributes))}
          disabled={!classes.length || !attributes.length}
        >
          <Plus size={12} />
          {S.ontology.ruleNew}
        </Button>
      )}
    </div>
  );
}
