import { useEffect, useState } from "react";
import { LayoutDashboard } from "lucide-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import {
  api,
  type AgentDecision,
  type AgentPrecedent,
  type AxiomViolation,
  type ReviewQueue,
  type ReviewTypeFilter,
  type OntologyDefect,
  type AlignmentItem,
  type EntityTypeView,
  type RelationTypeView,
  type ConflictItem,
  type FactReviewItem,
  type MergeLog,
  type PendingFactItem,
  type ReviewHistoryEvent,
  type ReviewItem,
  type ReviewSide,
  type ViolationResolution,
} from "../api";
import { parseDateInput } from "../time";
import { PendingFactRow, useCanDecide } from "./PendingFacts";
import { ReviewOverview } from "./ReviewOverview";
import { S } from "../i18n";
import { useKb, useKbId } from "../kb";
import { toast } from "../toast";
import {
  Button,
  CARD_ACTIONS,
  Checkbox,
  Chip,
  GroupLabel,
  Input,
  LinkButton,
  PageHeader,
  Pager,
  RAIL_CLS,
  RailItem,
  Segmented,
  SearchSelect,
  Status,
  cn,
  type ChipTone,
} from "../ui";

const DUP_PAGE = 6;
const FACT_PAGE = 10;
const MERGE_PAGE = 10;
const CONFLICT_PAGE = 8;

const ym = (iso: string | null) => (iso ? iso.slice(0, 7) : null);

/** `code` 或 `code|detail`。查不到就原样显示——存量行里还是旧的英文散文 */
function escalationText(reason: string): string {
  const [code, detail] = reason.split("|");
  const worded = S.review.escalated[code];
  if (!worded) return reason;
  // 执行闸门（0027）留下的：detail 是 `kind value`，按 kind 措辞，不把裸代码给人看
  if (code === "escalate_impact" && detail) {
    const [kind, ...rest] = detail.split(" ");
    const said = S.review.impact[kind]?.(rest.join(" "));
    if (said) return `${worded} — ${said}`;
  }
  return detail ? S.errDetail(worded, detail) : worded;
}

function dateRange(from: string | null, to: string | null): string | null {
  if (!from && !to) return null;
  return `${ym(from) ?? "…"} → ${ym(to) ?? S.review.ongoing}`;
}

function SideCard({ side }: { side: ReviewSide }) {
  return (
    <div className="flex-1 min-w-0">
      {/* **只写名字，不画那颗色点。**色点是画布上的记号：那里一屏几十个节点，
          颜色是唯一能一眼分开类的东西。这张卡上一共就两个实体，类名下一行
          白纸黑字写着（「Organization · 5 facts」），点再说一遍等于重复，
          而且把名字往右顶了一截。 */}
      <div className="mb-1 flex items-center gap-2">
        <span className="truncate text-body font-medium text-ink">
          {side.name}
        </span>
        {side.disambiguator && (
          <span className="text-small text-ink-2 truncate">
            · {side.disambiguator}
          </span>
        )}
      </div>
      <div className="text-small text-ink-2 mb-2">
        {side.type_label ?? S.graph.untyped} ·{" "}
        {S.review.factsCount(side.degree)}
      </div>
      {side.top_facts.length > 0 ? (
        <ul className="space-y-1">
          {side.top_facts.map((f, i) => (
            <li key={i} className="text-small text-ink-2 truncate">
              {f}
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-small text-ink-2">{S.review.noFacts}</p>
      )}
    </div>
  );
}

/** 两边都有类型且不一样：同名异义，合了就错。这是扫一屏重复项时最该一眼
 *  看到的一件事，所以提成标记，不让人去两张卡片里各读一行小字 */
function typesDiffer(item: ReviewItem): boolean {
  return (
    item.left.type_label !== null &&
    item.right.type_label !== null &&
    item.left.type_label !== item.right.type_label
  );
}

function DuplicateCard({
  item,
  busy,
  locked,
  picked,
  onPick,
  onDecide,
}: {
  item: ReviewItem;
  busy: boolean;
  /** agent 正在裁这一对（0025）：这几分钟里人不能动它，接口也会拒绝 */
  locked: boolean;
  /** 批量选中（#428）：勾在卡片左上，选了就跟着上面的批量按钮走 */
  picked: boolean;
  onPick: (picked: boolean) => void;
  /** 第二个参数是人写的那一句（0026）：什么让你这么定。可空 */
  onDecide: (action: "merge" | "keep", rationale?: string) => void;
}) {
  const reasonCode = item.reason?.split("|", 1)[0];
  const [why, setWhy] = useState("");

  return (
    <div className={cn("glass rounded-panel p-4", picked && "u-picked")}>
      {/* **为什么是这一对**，写在两边之前：等谁裁、类型对不对得上、agent 建议
          什么、凭什么说它们像。读完这一行再看下面两栏，才知道该盯什么。 */}
      <div className="mb-3 flex flex-wrap items-center gap-3">
        {locked ? (
          <Status tone="warn" pulse>
            {S.review.agentDeciding}
          </Status>
        ) : (
          <Status tone={item.stage === "human" ? "warn" : "neutral"}>
            {item.stage === "human"
              ? S.review.stageHuman
              : S.review.stageAdjudicating}
          </Status>
        )}
        {typesDiffer(item) && (
          <Chip tone="warn" title={S.review.typesDifferHint}>
            {S.review.typesDiffer(
              item.left.type_label ?? "",
              item.right.type_label ?? "",
            )}
          </Chip>
        )}
        {/* agent 的建议（0025）：底下的 Merge / Keep 就是对它的回答 */}
        {item.proposal && (
          <Chip tone="info" title={item.proposal.reason ?? undefined}>
            {S.review.agentSuggests(
              S.review.agentActions[item.proposal.action],
              Math.round(item.proposal.confidence * 100),
            )}
          </Chip>
        )}
        {reasonCode !== "namesake" && (
          <span className="text-small text-ink-2">
            {S.review.similarity(Math.round(item.score * 100))}
          </span>
        )}
        {item.reason && (
          <span className="truncate text-small text-ink-2">
            {escalationText(item.reason)}
          </span>
        )}
      </div>
      <div className="flex gap-4">
        <Checkbox
          className="shrink-0 self-start"
          checked={picked}
          disabled={busy || locked}
          onChange={(v) => onPick(v)}
          label={<span className="sr-only">{S.review.pickPair}</span>}
        />
        <SideCard side={item.left} />
        <div className="self-center text-ink-2 text-body shrink-0">≟</div>
        <SideCard side={item.right} />
      </div>
      {/* 页脚只剩动作，与别的卡同一副（CARD_ACTIONS）：左下，危险的排最后。
          「为什么是这一对」搬到卡片最上面去了——它是**读这张卡之前要知道的事**，
          不是决定之后的脚注。压在右下角的时候，人得先看完两边的事实，再把眼睛
          甩到对角去找「凭什么说它们像」。 */}
      <div className={cn(CARD_ACTIONS, "pt-3 border-t border-line")}>
        {/* 理由框（0026）：可不写；写了就跟着决定进台账，下一次先例带着它 */}
        <Input
          size="sm"
          className="w-56"
          placeholder={S.review.rationalePlaceholder}
          value={why}
          disabled={busy || locked}
          onChange={(e) => setWhy(e.target.value)}
        />
        <Button variant="primary" size="sm"
          disabled={busy || locked}
          onClick={() => onDecide("merge", why)}
        >
          {S.review.merge}
        </Button>
        <Button variant="secondary" size="sm"
          disabled={busy || locked}
          onClick={() => onDecide("keep", why)}
        >
          {S.review.keep}
        </Button>
      </div>
    </div>
  );
}

function FactRow({
  fact,
  busy,
  onConfirm,
  onReject,
}: {
  fact: FactReviewItem;
  busy: boolean;
  onConfirm: () => void;
  onReject: () => void;
}) {
  const range = dateRange(fact.valid_from, fact.valid_to);
  return (
    <div className="glass rounded-panel p-4">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-body font-medium text-ink">
          {fact.subject_name}
        </span>
        <span className="text-small text-ink-2">
          —{" "}
          <span
            className={
              fact.predicate_label === null
                ? "italic text-ink-2"
                : undefined
            }
          >
            {fact.predicate_label ?? S.graph.unknownPredicate}
          </span>{" "}
          →
        </span>
        <span className="text-body font-medium text-ink">
          {fact.object_name ?? "?"}
        </span>
        {range && <span className="text-small text-ink-2">({range})</span>}
        {/* 置信度是**一个数**，不是一个状态：与这一页另外三处置信度同一副
            素色数字。从前这里是一枚填色的琥珀胶囊，同一个数在同一页有两副样子 */}
        <span className="ml-auto u-num text-small text-ink-2 shrink-0">
          {S.review.confidence(Math.round(fact.confidence * 100))}
        </span>
      </div>
      {fact.quote && (
        <p className="mt-2 text-small text-ink-2 italic line-clamp-2">
          “{fact.quote}”
        </p>
      )}
      <div className={CARD_ACTIONS}>
        <Button variant="secondary" size="sm"
          disabled={busy}
          onClick={onConfirm}
        >
          {S.review.confirm}
        </Button>
        <Button variant="danger" size="sm"
          disabled={busy}
          onClick={onReject}
        >
          {S.review.reject}
        </Button>
      </div>
    </div>
  );
}

/** 时态冲突行：旧事实 vs 新事实，三个动作（Close old / Keep both / Reject new）。 */
function ConflictRow({
  conflict,
  busy,
  onResolve,
}: {
  conflict: ConflictItem;
  busy: boolean;
  onResolve: (
    action: "close" | "keep" | "reject_new",
    closeAt?: string,
    closeAtPrecision?: string,
  ) => void;
}) {
  const [closeAt, setCloseAt] = useState("");
  const c = conflict;
  const needsDate = !c.new_valid_from;
  // 写多少位就是多少精度（time.ts）：「2023-06」闭合在那个月，不编一个 1 日
  const closeParsed = parseDateInput(closeAt);
  const closeAtIso = closeParsed?.iso;

  return (
    <div className="glass rounded-panel p-4">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-body font-medium text-ink">{c.old_subject}</span>
        <span className="text-small text-ink-2">
          — {c.predicate_label} →
        </span>
        <span className="text-body font-medium text-ink">
          {c.old_object ?? "?"}
        </span>
        {c.old_valid_from && (
          <span className="u-num text-small text-ink-2">
            ({S.review.conflictSince(c.old_valid_from.slice(0, 10))})
          </span>
        )}
        <span className="text-small text-ink-2">{S.review.conflictVs}</span>
        <span className="text-body font-medium text-ink">{c.new_subject}</span>
        <span className="text-small text-ink-2">
          — {c.predicate_label} →
        </span>
        <span className="text-body font-medium text-ink">
          {c.new_object ?? "?"}
        </span>
        {c.new_valid_from && (
          <span className="u-num text-small text-ink-2">
            ({S.review.conflictSince(c.new_valid_from.slice(0, 10))})
          </span>
        )}
        <Status tone="warn" className="ml-auto shrink-0">
          {S.review.conflictReason[c.reason] ?? c.reason}
        </Status>
      </div>
      <div className={CARD_ACTIONS}>
        <Button variant="secondary" size="sm"
          disabled={busy}
          onClick={() => onResolve("keep")}
        >
          {S.review.keepBoth}
        </Button>
        {needsDate && (
          <Input size="sm" className="u-num w-28 text-center"
            placeholder={S.review.closeAtPlaceholder}
            value={closeAt}
            onChange={(e) => setCloseAt(e.target.value)}
          />
        )}
        <Button variant="secondary" size="sm"
          disabled={busy || (needsDate && !closeAtIso)}
          onClick={() => onResolve("close", closeAtIso, closeParsed?.precision)}
        >
          {c.new_valid_from
            ? S.review.closeOldAt(c.new_valid_from.slice(0, 10))
            : S.review.closeOld}
        </Button>
        <Button variant="danger" size="sm"
          disabled={busy}
          onClick={() => onResolve("reject_new")}
        >
          {S.review.rejectNew}
        </Button>
      </div>
    </div>
  );
}

/** "文档新版没再提"的事实行：Reject（抽取错误）或 Close at date（这事结束了）。 */
function UnconfirmedRow({
  fact,
  busy,
  onReject,
  onClose,
}: {
  fact: FactReviewItem;
  busy: boolean;
  onReject: () => void;
  onClose: (validTo: string, precision: string) => void;
}) {
  const [closeAt, setCloseAt] = useState("");
  const closeParsed = parseDateInput(closeAt);
  const closeAtIso = closeParsed?.iso;
  const range = dateRange(fact.valid_from, fact.valid_to);

  return (
    <div className="glass rounded-panel p-4">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-body font-medium text-ink">
          {fact.subject_name}
        </span>
        <span className="text-small text-ink-2">
          —{" "}
          <span
            className={
              fact.predicate_label === null
                ? "italic text-ink-2"
                : undefined
            }
          >
            {fact.predicate_label ?? S.graph.unknownPredicate}
          </span>{" "}
          →
        </span>
        <span className="text-body font-medium text-ink">
          {fact.object_name ?? "?"}
        </span>
        {range && (
          <span className="u-num text-small text-ink-2">({range})</span>
        )}
      </div>
      {fact.quote && (
        <p className="mt-2 text-small text-ink-2 italic line-clamp-2">
          “{fact.quote}”
        </p>
      )}
      <div className={CARD_ACTIONS}>
        <Input size="sm" className="u-num w-28 text-center"
          placeholder={S.review.closeAtPlaceholder}
          value={closeAt}
          onChange={(e) => setCloseAt(e.target.value)}
        />
        <Button variant="secondary" size="sm"
          disabled={busy || !closeAtIso}
          onClick={() =>
            closeParsed && onClose(closeParsed.iso, closeParsed.precision)
          }
        >
          {closeAt.trim()
            ? S.review.closeFactAt(closeAt.trim())
            : S.review.closeFact}
        </Button>
        <Button variant="danger" size="sm"
          disabled={busy}
          onClick={onReject}
        >
          {S.review.reject}
        </Button>
      </div>
    </div>
  );
}

function MergeRow({
  merge,
  busy,
  onRevert,
}: {
  merge: MergeLog;
  busy: boolean;
  onRevert: () => void;
}) {
  return (
    <div className="glass rounded-panel px-4 py-3 flex items-center gap-3">
      <div className="min-w-0 flex-1">
        <div className="text-body text-ink-2 truncate">
          <span className="text-ink-2">{merge.source_name}</span>
          <span className="text-ink-2"> → </span>
          <span className="text-ink">{merge.target_name}</span>
        </div>
        <div className="text-small text-ink-2 truncate">
          {merge.merged_by_name
            ? S.review.mergedBy(merge.merged_by_name)
            : S.review.mergedByAi}
          {" · "}
          {merge.created_at.slice(0, 10)}
          {merge.reason ? ` · ${escalationText(merge.reason)}` : ""}
        </div>
      </div>
      {merge.reverted_at ? (
        <Status className="shrink-0">{S.review.reverted}</Status>
      ) : (
        <Button variant="secondary" size="sm" className="shrink-0"
          disabled={busy}
          onClick={onRevert}
        >
          {S.review.revert}
        </Button>
      )}
    </div>
  );
}

/* ---------- agent 的一笔（0025） ---------- */

const AGENT_ACTION_TONE: Record<AgentDecision["action"], ChipTone> = {
  merge: "violet",
  keep: "neutral",
  unsure: "warn",
};

const AGENT_STATUS_TONE: Record<AgentDecision["status"], ChipTone> = {
  proposed: "warn",
  applied: "info",
  accepted: "success",
  overridden: "neutral",
  reverted: "danger",
  superseded: "neutral",
};

/** 一条先例写成一句话；类型对的习惯是汇总，单独一句 */
function precedentText(p: AgentPrecedent): string {
  if (p.family === "type_pair")
    return S.review.agentPrecedentHabit(p.merged, p.kept, p.reverted);
  const verb =
    p.action === "merge.revert"
      ? S.review.agentPrecedentReverted
      : p.action === "review.keep"
        ? S.review.agentPrecedentKept
        : S.review.agentPrecedentMerged;
  const line = `${p.left} ≟ ${p.right} · ${verb} · ${p.at.slice(0, 10)}`;
  // 人写的那一句（0026）跟在后面：先例不只是结果
  return p.why ? `${line} · “${p.why}”` : line;
}

function AgentRow({
  d,
  busy,
  onAnswer,
}: {
  d: AgentDecision;
  busy: boolean;
  onAnswer: (action: "merge" | "keep" | "revert", rationale?: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [why, setWhy] = useState("");
  const precedents = d.precedents ?? [];
  const trace = d.trace ?? [];
  const hasDetail = precedents.length > 0 || trace.length > 0;
  return (
    <div className="glass rounded-panel px-4 py-3">
      <div className="flex items-center gap-3">
        <Status tone={AGENT_ACTION_TONE[d.action]}>{S.review.agentActions[d.action]}</Status>
        <span className="text-body text-ink-2 truncate min-w-0">
          {d.left ?? "?"} ≟ {d.right ?? "?"}
        </span>
        <span className="u-num text-small text-ink-2 shrink-0">
          {Math.round(d.confidence * 100)}%
        </span>
        <Status tone={AGENT_STATUS_TONE[d.status]} className="ml-auto shrink-0">
          {S.review.agentStatus[d.status]}
        </Status>
      </div>
      {/* defer 留下的问题（第二刀）：这是给人看的正文，不是注脚 */}
      {d.question && (
        <p className="mt-1 text-body text-ink">
          <span className="text-ink-2">{S.review.agentAsks} </span>
          {d.question}
        </p>
      )}
      {(d.reason || hasDetail) && (
        <div className="mt-1 flex items-center gap-3 text-small text-ink-2">
          {d.reason && <span className="truncate min-w-0">{d.reason}</span>}
          {hasDetail && (
            <LinkButton className="shrink-0" onClick={() => setOpen((v) => !v)}>
              {[
                precedents.length > 0 ? S.review.agentPrecedents(precedents.length) : null,
                trace.length > 0 ? S.review.agentLookups(trace.length) : null,
              ]
                .filter(Boolean)
                .join(" · ")}
            </LinkButton>
          )}
        </div>
      )}
      {open && hasDetail && (
        <ul className="mt-2 space-y-1 border-t border-line pt-2 text-small text-ink-2">
          {precedents.map((p, i) => (
            <li key={`p${i}`} className="truncate">
              {precedentText(p)}
            </li>
          ))}
          {/* 轨迹就是解释：它查了什么，一行一次 */}
          {trace.map((t, i) => (
            <li key={`t${i}`} className="truncate">
              {S.review.agentLookedAt} {t.note}
            </li>
          ))}
        </ul>
      )}
      <div className="mt-2 flex items-center gap-2">
        <span className="text-fine text-ink-2">
          {d.decided_by_name
            ? S.review.agentAnsweredBy(d.decided_by_name, (d.decided_at ?? d.created_at).slice(0, 10))
            : d.created_at.slice(0, 10)}
        </span>
        <div className="ml-auto flex items-center gap-2 shrink-0">
          {/* 回答 agent 也能带一句理由（0026）——它走的正是人的裁决路径 */}
          {(d.status === "proposed" || d.status === "applied") && (
            <Input
              size="sm"
              className="w-56"
              placeholder={S.review.rationalePlaceholder}
              value={why}
              disabled={busy}
              onChange={(e) => setWhy(e.target.value)}
            />
          )}
          {d.status === "proposed" && (
            <>
              <Button variant="secondary" size="sm" disabled={busy} onClick={() => onAnswer("keep", why)}>
                {S.review.keep}
              </Button>
              <Button variant="primary" size="sm" disabled={busy} onClick={() => onAnswer("merge", why)}>
                {S.review.merge}
              </Button>
            </>
          )}
          {d.status === "applied" && d.action === "merge" && (
            <Button variant="secondary" size="sm" disabled={busy} onClick={() => onAnswer("revert", why)}>
              {S.review.revert}
            </Button>
          )}
          {d.status === "applied" && d.action === "keep" && (
            <Button variant="secondary" size="sm" disabled={busy} onClick={() => onAnswer("merge", why)}>
              {S.review.merge}
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}

/* ---------- 决策台账行 ---------- */

const DECISION_TONE: Record<string, ChipTone> = {
  "review.merge": "violet",
  "merge.manual": "violet",
  "review.keep": "neutral",
  "fact.confirm": "success",
  "fact.reject": "danger",
  "conflict.reject_new": "danger",
  "fact.close": "info",
  "conflict.close_old": "info",
  "conflict.keep_both": "neutral",
  "merge.revert": "warn",
};

function DecisionRow({ e }: { e: ReviewHistoryEvent }) {
  // detail 是决策时的自包含快照——不 join 活数据，事实删了台账也完整
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const d = e.detail as any;
  let text: string;
  if (e.action.startsWith("review.")) text = `${d.left} ≟ ${d.right}`;
  else if (e.action.startsWith("fact."))
    text = `${d.subject} — ${d.predicate ?? "?"} → ${d.object ?? "?"}`;
  else if (e.action.startsWith("conflict."))
    text = `${d.old_subject} — ${d.predicate} → ${d.old_object ?? "?"} · vs · ${
      d.new_object ?? d.new_subject
    }`;
  else text = `${d.source} → ${d.target}`;

  return (
    <div className="glass rounded-panel px-4 py-3 flex items-center gap-3">
      <Chip tone={DECISION_TONE[e.action] ?? "neutral"}>
        {S.review.decisionActions[e.action] ?? e.action}
      </Chip>
      <span className="text-body text-ink-2 truncate min-w-0">{text}</span>
      {/* 人（或模型）写的那一句（0026）；老行没有 */}
      {typeof d.why === "string" && d.why && (
        <span className="text-small text-ink-2 truncate min-w-0">
          “{d.why}”
        </span>
      )}
      {typeof d.confidence === "number" && (
        <span className="u-num text-small text-ink-2 shrink-0">
          {Math.round(d.confidence * 100)}%
        </span>
      )}
      {typeof d.valid_to === "string" && (
        <span className="u-num text-small text-ink-2 shrink-0">
          → {d.valid_to.slice(0, 10)}
        </span>
      )}
      <span className="ml-auto shrink-0 text-small text-ink-2">
        {e.actor_name ?? S.review.aiActor}
        {" · "}
        <span className="u-num">{e.created_at.slice(0, 10)}</span>
      </span>
    </div>
  );
}

/** 一条待表态的数据映射口径（0011）。
 *
 * 展示的重点是**「这个数怎么算」**——SQL / 表达式 / 表名按这个优先级取一个，
 * 因为人要判断的正是它对不对。概念名与源是身份，unit 是答里必须带的量纲。 */
/** 本体自己的一处自相矛盾。**两个按钮而不是三个**——这一档压根没看数据，
 *  所以没有「数据错了」这条出路，只能是「我去改了本体」或「先放着」。 */
function voteText(v: { property: string; direction: string } | string | null | undefined): string {
  if (v === undefined || v === null) return S.review.alignmentNone;
  if (typeof v === "string") return v;
  return `${v.property} · ${v.direction === "reverse" ? S.review.alignmentReverse : S.review.alignmentForward}`;
}

/** 一条短语签名：短语、两端的类、例句、两票；人选属性与方向，或「没有」 */
function AlignmentPhraseRow({
  item,
  properties,
  busy,
  onDecide,
}: {
  item: Extract<AlignmentItem, { kind: "phrase" }>;
  properties: RelationTypeView[];
  busy: boolean;
  onDecide: (property: string | null, direction: "forward" | "reverse") => void;
}) {
  const first = item.votes?.first ?? null;
  const second = item.votes?.second ?? null;
  const [property, setProperty] = useState<string>(first?.property ?? second?.property ?? "");
  const [direction, setDirection] = useState<"forward" | "reverse">(
    first?.direction ?? second?.direction ?? "forward",
  );
  // 字面值当宾语的签名只配属性（attribute），两样东西之间的只配关系（relation）
  const fitting = properties.filter((p) =>
    item.object_is_value ? p.kind === "attribute" : p.kind === "relation",
  );
  return (
    <div className="glass rounded-panel p-3">
      <div className="flex items-baseline gap-2 flex-wrap">
        <span className="text-body text-ink">“{item.phrase}”</span>
        <span className="text-small text-ink-2">
          {item.subject_class ?? "?"} → {item.object_is_value ? S.review.alignmentValue : (item.object_class ?? "?")}
        </span>
        <span className="text-small text-ink-2">{S.review.alignmentStatements(item.statement_count)}</span>
      </div>
      {item.examples.length > 0 && (
        <div className="mt-1 space-y-1 text-small text-ink-2">
          {item.examples.map((e, i) => (
            <div key={i}>{e}</div>
          ))}
        </div>
      )}
      <div className="mt-1 text-small text-ink-2">
        {S.review.alignmentVotes(voteText(first), voteText(second))}
      </div>
      <div className={CARD_ACTIONS}>
        <SearchSelect
          size="sm"
          className="w-56"
          value={property}
          onChange={setProperty}
          options={[
            { value: "", label: S.review.alignmentNone },
            ...fitting.map((p) => ({ value: p.key, label: p.label })),
          ]}
        />
        {property && !item.object_is_value && (
          <Segmented
            size="sm"
            value={direction}
            onChange={setDirection}
            options={[
              { value: "forward", label: S.review.alignmentForward },
              { value: "reverse", label: S.review.alignmentReverse },
            ]}
          />
        )}
        <Button size="sm" disabled={busy} onClick={() => onDecide(property || null, direction)}>
          {property ? S.review.alignmentBind : S.review.alignmentLeaveOpen}
        </Button>
      </div>
    </div>
  );
}

/** 一个类别词：词、写法、例名、它们参与的短语、两票；人选类，或「没有」 */
function AlignmentKindWordRow({
  item,
  classes,
  busy,
  onDecide,
}: {
  item: Extract<AlignmentItem, { kind: "kind_word" }>;
  classes: EntityTypeView[];
  busy: boolean;
  onDecide: (cls: string | null) => void;
}) {
  const first = item.votes?.first ?? null;
  const second = item.votes?.second ?? null;
  const [cls, setCls] = useState<string>(first ?? second ?? "");
  return (
    <div className="glass rounded-panel p-3">
      <div className="flex items-baseline gap-2 flex-wrap">
        <span className="text-body text-ink">{item.kind_word}</span>
        {item.words.length > 0 && (
          <span className="text-small text-ink-2">{item.words.join(" · ")}</span>
        )}
        <span className="text-small text-ink-2">{S.review.alignmentEntities(item.entity_count)}</span>
      </div>
      {item.examples.length > 0 && (
        <div className="mt-1 text-small text-ink-2">{item.examples.join(" · ")}</div>
      )}
      {item.phrases.length > 0 && (
        <div className="mt-1 text-small text-ink-2">{item.phrases.map((p) => `—${p}→`).join("  ")}</div>
      )}
      <div className="mt-1 text-small text-ink-2">
        {S.review.alignmentVotes(voteText(first), voteText(second))}
      </div>
      <div className={CARD_ACTIONS}>
        <SearchSelect
          size="sm"
          className="w-56"
          value={cls}
          onChange={setCls}
          options={[
            { value: "", label: S.review.alignmentNone },
            ...classes.map((c) => ({ value: c.key, label: c.label })),
          ]}
        />
        <Button size="sm" disabled={busy} onClick={() => onDecide(cls || null)}>
          {cls ? S.review.alignmentBind : S.review.alignmentLeaveOpen}
        </Button>
      </div>
    </div>
  );
}

function DefectRow({
  defect: d,
  busy,
  onDecide,
}: {
  defect: OntologyDefect;
  busy: boolean;
  onDecide: (resolution: "fixed" | "accepted") => void;
}) {
  const what = {
    symmetric_and_asymmetric: S.review.defectSymAsym,
    transitive_and_functional: S.review.defectTransFunc,
    subclass_cycle: S.review.defectCycle,
    disjoint_with_ancestor: S.review.defectDisjointAncestor,
    inherits_disjoint: S.review.defectInheritsDisjoint,
    inverse_of_itself: S.review.defectInverseSelf,
    inverse_not_mutual: S.review.defectInverseNotMutual,
    sub_property_cycle: S.review.defectSubPropertyCycle,
    rules_disagree: S.review.defectRulesDisagree,
  }[d.kind];
  const rules = d.kind === "rules_disagree" ? (d.detail.rules ?? []) : [];
  // 后两类的后果值得写出来：不可满足的类不会报错，它只是永远空着
  const unsatisfiable =
    d.kind === "disjoint_with_ancestor" || d.kind === "inherits_disjoint";
  return (
    <div className="glass rounded-panel p-3">
      <div className="flex items-baseline gap-2 flex-wrap">
        <span className="text-body text-danger">{what}</span>
        {d.subject_label && (
          <span className="text-small text-ink-2">{d.subject_label}</span>
        )}
        {d.other_label && (
          <span className="text-small text-ink-2">↔ {d.other_label}</span>
        )}
      </div>
      {d.path_labels.length > 0 && (
        <div className="mt-1 text-small text-ink-2">
          {d.path_labels.join(" → ")} → {d.path_labels[0]}
        </div>
      )}
      {unsatisfiable && (
        <p className="mt-1 text-small text-ink-2">
          {S.review.defectNeverInstantiable}
        </p>
      )}
      {rules.length > 0 && (
        <div className="mt-1 space-y-1 text-small text-ink-2">
          <div>{S.review.rulesDisagreeCount(d.detail.count ?? 0)}</div>
          {rules.map((r, i) => (
            <div key={i}>
              <div className="text-ink-2">
                {S.review.rulesDisagreeRule(r.rule_a, r.via_a, r.rule_b, r.via_b, r.axiom)}
              </div>
              {r.examples.map(([x, y], j) => (
                <div key={j} className="pl-3 text-ink-2">
                  {x} <span className="text-ink-2">·</span> {y}
                </div>
              ))}
            </div>
          ))}
        </div>
      )}
      <div className={CARD_ACTIONS}>
        <Button variant="secondary" size="sm"
          disabled={busy}
          onClick={() => onDecide("accepted")}
        >
          {S.review.defectAccepted}
        </Button>
        <Button variant="primary" size="sm"
          disabled={busy}
          onClick={() => onDecide("fixed")}
        >
          {S.review.defectFixed}
        </Button>
      </div>
    </div>
  );
}

/** 一处公理违规。**三个按钮而不是两个**——第三个是这一档独有的出路：
 *  矛盾可能出在定义上（用户导的本体把某个属性声明成反对称，而他的语料里
 *  那关系其实双向），这时该改的是本体，不是二十条事实。 */
/** 裁决的附加参数：闭合日期（fact_closed）、撤哪条（fact_retracted） */
type DecideOpts = { closeAt?: string; closeAtPrecision?: string; factId?: string };

function ViolationRow({
  violation: v,
  busy,
  onDecide,
  onDuplicates,
  onOntology,
}: {
  violation: AxiomViolation;
  busy: boolean;
  onDecide: (resolution: ViolationResolution, opts?: DecideOpts) => void;
  onDuplicates: () => void;
  onOntology: () => void;
}) {
  const what = {
    self_loop: S.review.violationSelfLoop,
    asymmetry: S.review.violationAsymmetry,
    cycle: S.review.violationCycle,
    functional: S.review.violationFunctional,
    inverse_functional: S.review.violationInverseFunctional,
    signature: S.review.violationSignature,
    derived_contradiction: S.review.violationDerived,
  }[v.kind];
  if (v.kind === "derived_contradiction") {
    return <ContradictionRow {...{ v, what, busy, onDecide, onDuplicates, onOntology }} />;
  }
  // 自反那一类两条事实是同一条——显示一遍就够，显示两遍像个 bug
  const single = v.left_fact === v.right_fact;
  // 「数据错了」要撤具体哪一条：环逐条列，双事实的两条各一个按钮，单事实的不用问（#202）
  const facts =
    v.path.length > 0
      ? v.path
      : single
        ? [{ id: v.left_fact, text: v.left_text }]
        : [
            { id: v.left_fact, text: v.left_text },
            { id: v.right_fact, text: v.right_text },
          ];
  return (
    <div className="glass rounded-panel p-3">
      <div className="flex items-baseline gap-2 flex-wrap">
        <span className="text-body text-warn">{what}</span>
        {v.predicate && (
          <span className="text-fine text-ink-2">
            {S.review.violationVia(v.predicate)}
          </span>
        )}
        {v.kind === "cycle" && v.path_len > 0 && (
          <span className="text-fine text-ink-2">
            {S.review.violationPath(v.path_len)}
          </span>
        )}
      </div>
      <div className="mt-2 space-y-1">
        {facts.map((f) => (
          <div key={f.id} className="flex items-center gap-2">
            <span className="text-small text-ink-2 min-w-0 flex-1">{f.text}</span>
            {!single && (
              <Button variant="secondary" size="sm" className="shrink-0"
                disabled={busy}
                title={S.review.retractThisHint}
                onClick={() => onDecide("fact_retracted", { factId: f.id })}
              >
                {S.review.retractThis}
              </Button>
            )}
          </div>
        ))}
      </div>
      <div className={CARD_ACTIONS}>
        <Button variant="secondary" size="sm"
          disabled={busy}
          onClick={() => onDecide("accepted")}
        >
          {S.review.acceptBoth(facts.length)}
        </Button>
        <Button variant="secondary" size="sm"
          disabled={busy}
          onClick={() => onDecide("axiom_relaxed")}
        >
          {S.review.relaxAxiom}
        </Button>
        {single && (
          <Button variant="primary" size="sm"
            disabled={busy}
            onClick={() => onDecide("fact_retracted", { factId: v.left_fact })}
          >
            {S.review.retractFact}
          </Button>
        )}
      </div>
    </div>
  );
}

/**
 * 派生撞上断言（0017）：卡片是一次审核，线索指向上游的错——旧断言该闭合、
 * 两个同名实体其实是一个、抽取本来就没把握。修法就在卡片上，端点替人执行。
 */
function ContradictionRow({
  v,
  what,
  busy,
  onDecide,
  onDuplicates,
  onOntology,
}: {
  v: AxiomViolation;
  what: string;
  busy: boolean;
  onDecide: (resolution: ViolationResolution, opts?: DecideOpts) => void;
  onDuplicates: () => void;
  onOntology: () => void;
}) {
  const [closeAt, setCloseAt] = useState("");
  const d = v.detail;
  const hint =
    v.hint === "stale"
      ? S.review.hintStale
      : v.hint === "duplicate"
        ? S.review.hintDuplicate
        : v.hint === "unsure"
          ? S.review.hintUnsure
          : S.review.hintReadBoth;
  return (
    <div className="glass rounded-panel p-3 border border-[color-mix(in_srgb,var(--u-contest)_35%,transparent)]">
      <div className="flex items-baseline gap-2 flex-wrap">
        <span className="text-body text-contest">{what}</span>
        {v.predicate && (
          <span className="text-fine text-ink-2">
            {S.review.violationVia(v.predicate)}
          </span>
        )}
      </div>
      <div className="mt-2 space-y-1">
        <div className="text-small text-ink-2">
          {S.review.derivedLine(d.subject ?? "?", d.predicate ?? "?", d.object ?? "?")}
          {d.rule && d.via_label && (
            <span className="ml-2 text-ink-2">
              {S.review.derivedBy(d.rule, d.via_label)}
            </span>
          )}
        </div>
        <div className="text-small text-ink-2">
          {S.review.assertedLine(v.left_text)}
        </div>
      </div>
      <p className="mt-2 text-small text-ink-2">{hint}</p>
      <div className={CARD_ACTIONS}>
        {/* 不用日期选择器：它逼人给出一个日，而「那年结束的」正是这里常见的答案。
            写多少位就是多少精度（time.ts） */}
        <Input size="sm" className="u-num w-28 text-center"
          placeholder={S.review.closeAtPlaceholder}
          value={closeAt}
          title={S.review.closeAssertion}
          onChange={(e) => setCloseAt(e.target.value)}
        />
        <Button variant="primary" size="sm"
          disabled={busy || !parseDateInput(closeAt)}
          onClick={() => {
            const parsed = parseDateInput(closeAt);
            if (parsed) {
              onDecide("fact_closed", {
                closeAt: parsed.iso,
                closeAtPrecision: parsed.precision,
              });
            }
          }}
        >
          {S.review.closeAssertion}
        </Button>
        <Button variant="secondary" size="sm"
          disabled={busy}
          onClick={() => onDecide("fact_retracted")}
        >
          {S.review.retractAssertion}
        </Button>
        <Button variant="secondary" size="sm"
          disabled={busy}
          onClick={onDuplicates}
        >
          {S.review.seeDuplicates}
        </Button>
        <Button variant="secondary" size="sm"
          disabled={busy}
          onClick={onOntology}
        >
          {S.review.openOntology}
        </Button>
        <Button variant="secondary" size="sm"
          disabled={busy}
          onClick={() => onDecide("accepted")}
        >
          {S.review.letBothStand}
        </Button>
      </div>
    </div>
  );
}

/* ---------- 页面：左栏分类 + 单类内容区 ---------- */

type Sel =
  // 总览（#377）：落地页。回答的是「有多少在等、等了多久、队列在消还是在涨」，
  // 不是任何一档队列
  | "overview"
  // 记忆抽出、等人点头的事实（0015）。排第一：它是人自己说的话，
  // 而且在这一档里的东西**还没进图**——别处每一档审的都是已经在图上的
  | "pending"
  | "duplicates"
  | "conflicts"
  | "unconfirmed"
  | "lowconf"
  // 公理违规（0002 R0）。**与 conflicts 分开**：那一档问「哪条对」，
  // 这一档还可能答「公理写错了」——出路不同
  | "violations"
  // 本体自己的自相矛盾。**与 violations 分开**：那一档看事实，这一档只看定义
  | "defects"
  // 对齐器两票不一致的签名与类别词（#725，0044 决定 3）：问的是「这个说法是本体的哪个属性」
  | "alignment"
  // agent 的每一笔（0025）：建议等人答，自动裁的可撤。它不是七档之一——
  // 七档问「这条知识对不对」，这一档问「机器替你办的对不对」
  | "agent"
  | "decisions"
  | "merges";

/** 走服务端分页的那几档（决策台账另有自己的接口） */
const QUEUE_FETCHED: ReviewQueue[] = [
  "pending",
  "duplicates",
  "conflicts",
  "unconfirmed",
  "lowconf",
  "violations",
  "defects",
  "alignment",
  "merges",
  "agent",
];

const QUEUE_ORDER: Sel[] = [
  "pending",
  "duplicates",
  "conflicts",
  "unconfirmed",
  "lowconf",
  "violations",
  "defects",
  "alignment",
];
/** 有内容区、要翻页的那些档——总览不翻页 */
type Paged = Exclude<Sel, "overview">;

const PAGE_SIZE: Record<Paged, number> = {
  pending: FACT_PAGE,
  duplicates: DUP_PAGE,
  conflicts: CONFLICT_PAGE,
  unconfirmed: FACT_PAGE,
  lowconf: FACT_PAGE,
  violations: FACT_PAGE,
  defects: FACT_PAGE,
  alignment: FACT_PAGE,
  merges: MERGE_PAGE,
  decisions: 20,
  agent: 20,
};

function RailHeader({ label }: { label: string }) {
  return (
    // 文字从 20 起，与行里的图标同一条线（盒 12 + 行内 8）
    <GroupLabel className="mx-2 px-3 pt-4 pb-2">{label}</GroupLabel>
  );
}


export function Review() {
  const kbId = useKbId();
  const { kb } = useKb();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  // 面板的争议 chip 带着 queue / item 跳过来：先落到那一档，再把那张卡点亮
  const search = useSearch({ from: "/app/kb/$kbId/review" });
  // 深链接认七档与 agent 那一档（0025 的告警会指过来）；别的落到总览
  const [sel, setSel] = useState<Sel | null>(
    QUEUE_ORDER.includes(search.queue as Sel) || search.queue === "agent"
      ? (search.queue as Sel)
      : null,
  );
  const [page, setPage] = useState(0);
  // 重复项的类型筛选与批量选中（#428）。选中集合按页清：翻页、换档、换筛选
  // 之后勾着的东西已经不在眼前，留着会让「合并所选」合掉看不见的东西
  const [types, setTypes] = useState<ReviewTypeFilter>("any");
  const [picked, setPicked] = useState<Set<string>>(() => new Set());
  const [batchWhy, setBatchWhy] = useState("");
  useEffect(() => setPicked(new Set()), [page, sel, types]);

  // 队列变化经 SSE 事件流推送（useKbEvents 挂在 Shell），无需轮询。
  //
  // **按分档 + 页码取**：从前一次把八个队列全端回来、每档 100 条、客户端分页，
  // 于是左栏的徽标是截断后的数字，第十一页之后的东西界面上不存在。现在计数
  // 每次都回（服务端 COUNT，不受一页多少条影响），内容只回当前这一档的一页。
  const queueSel: ReviewQueue = QUEUE_FETCHED.includes(
    (sel ?? "duplicates") as ReviewQueue,
  )
    ? ((sel ?? "duplicates") as ReviewQueue)
    : "duplicates";
  const typesForQuery = queueSel === "duplicates" ? types : "any";
  const review = useQuery({
    queryKey: ["review", kb?.id, queueSel, page, typesForQuery],
    queryFn: () =>
      api.review(
        kb!.id,
        queueSel,
        PAGE_SIZE[queueSel as Paged],
        page * PAGE_SIZE[queueSel as Paged],
        typesForQuery,
      ),
    enabled: !!kb,
    // 翻页时别把上一页闪成空白——计数与骨架都还在，只有条目在换
    placeholderData: (prev) => prev,
  });
  // 决策台账：服务端分页，仅选中时拉取
  const history = useQuery({
    queryKey: ["reviewHistory", kb?.id, page],
    queryFn: () => api.reviewHistory(kb!.id, page),
    enabled: !!kb && sel === "decisions",
  });

  // 总览：只在落在它上面时拉；每一次决定之后连它一起作废——它数的正是这些
  const summary = useQuery({
    queryKey: ["reviewSummary", kb?.id],
    queryFn: () => api.reviewSummary(kb!.id),
    enabled: !!kb && (sel ?? "overview") === "overview",
  });

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["review", kb?.id] });
    queryClient.invalidateQueries({ queryKey: ["reviewSummary", kb?.id] });
    queryClient.invalidateQueries({ queryKey: ["reviewHistory", kb?.id] });
    queryClient.invalidateQueries({ queryKey: ["graph"] });
  };

  const decide = useMutation({
    mutationFn: ({
      id,
      action,
      rationale,
    }: {
      id: string;
      action: "merge" | "keep";
      rationale?: string;
    }) => api.decideReview(kb!.id, id, action, rationale),
    onSettled: invalidate,
  });
  // 回答 agent 的一笔（0025）：走人的裁决路径，成为下一轮的先例
  const agentAnswer = useMutation({
    mutationFn: ({
      id,
      action,
      rationale,
    }: {
      id: string;
      action: "merge" | "keep" | "revert";
      rationale?: string;
    }) => api.agentAnswer(kb!.id, id, action, rationale),
    onError: (e) => toast.error((e as Error).message),
    onSettled: invalidate,
  });
  // 批量裁决：一批一个动作，回来逐条说成没成；没成的留在列表里，成了的消失
  const batch = useMutation({
    mutationFn: ({
      ids,
      action,
      rationale,
    }: {
      ids: string[];
      action: "merge" | "keep";
      rationale?: string;
    }) => api.reviewBatch(kb!.id, ids, action, rationale),
    onSuccess: (r) => {
      const failed = r.outcomes.filter((o) => o.error).length;
      if (failed > 0) toast.error(S.review.batchDone(r.decided, failed));
      else toast.success(S.review.batchDone(r.decided, 0));
    },
    onError: (e) => toast.error((e as Error).message),
    onSettled: () => {
      setPicked(new Set());
      setBatchWhy("");
      invalidate();
    },
  });
  const factAction = useMutation({
    mutationFn: ({
      id,
      action,
    }: {
      id: string;
      action: "confirm" | "reject";
    }) =>
      action === "confirm"
        ? api.confirmFact(kb!.id, id)
        : api.rejectFact(kb!.id, id),
    onSettled: invalidate,
  });
  // 等人点头的事实（0015）：确认进账本，驳回记进 rejected_facts
  // 点头是写图的动作：Viewer 看得见提议、看不见按钮（与服务端 Editor 门槛同口径）
  const canDecidePending = useCanDecide(kb?.id);
  const pendingAction = useMutation({
    mutationFn: ({
      id,
      action,
    }: {
      id: string;
      action: "confirm" | "reject";
    }) => api.decidePending(kb!.id, id, action),
    onSettled: () => {
      invalidate();
      queryClient.invalidateQueries({ queryKey: ["pending", kb?.id] });
    },
  });
  const defectAction = useMutation({
    mutationFn: ({
      id,
      resolution,
    }: {
      id: string;
      resolution: "fixed" | "accepted";
    }) => api.decideDefect(kb!.id, id, resolution),
    onSettled: invalidate,
  });
  const alignmentPhraseAction = useMutation({
    mutationFn: ({
      id,
      property,
      direction,
    }: {
      id: string;
      property: string | null;
      direction: "forward" | "reverse";
    }) => api.decideAlignmentPhrase(kb!.id, id, property, direction),
    onSuccess: (r) => {
      if (r.typed.added + r.typed.merged + r.typed.retired > 0) {
        toast.success(S.review.alignmentTyped(r.typed.added + r.typed.merged, r.typed.retired));
      }
    },
    onSettled: invalidate,
  });
  const alignmentKindWordAction = useMutation({
    mutationFn: ({ kindWord, cls }: { kindWord: string; cls: string | null }) =>
      api.decideAlignmentKindWord(kb!.id, kindWord, cls),
    onSettled: invalidate,
  });
  const violationAction = useMutation({
    mutationFn: ({
      id,
      resolution,
      opts,
    }: {
      id: string;
      resolution: ViolationResolution;
      opts?: DecideOpts;
    }) => api.decideViolation(kb!.id, id, resolution, opts),
    onSettled: invalidate,
  });
  // 检查是同步的纯计算,所以直接 mutate 不排队。跑完把报告留在按钮旁边——
  // **零和零不一样**：没有公理时要说「无从判起」,不能说「未发现矛盾」
  const runCheck = useMutation({
    mutationFn: () => api.runConsistencyCheck(kb!.id),
    onSettled: invalidate,
  });
  const revert = useMutation({
    mutationFn: (mergeId: string) => api.revertMerge(kb!.id, mergeId),
    onSettled: invalidate,
  });
  const conflictAction = useMutation({
    mutationFn: ({
      id,
      action,
      closeAt,
      closeAtPrecision,
    }: {
      id: string;
      action: "close" | "keep" | "reject_new";
      closeAt?: string;
      closeAtPrecision?: string;
    }) =>
      api.resolveConflict(kb!.id, id, {
        action,
        close_at: closeAt,
        close_at_precision: closeAtPrecision,
      }),
    onSettled: invalidate,
  });

  const closeFactAction = useMutation({
    mutationFn: ({
      id,
      validTo,
      precision,
    }: {
      id: string;
      validTo: string;
      precision: string;
    }) => api.closeFact(kb!.id, id, validTo, precision),
    onSettled: invalidate,
  });

  // **徽标读服务端的 COUNT，不读列表长度。** 这是从前那个「库里 164、界面写
  // 100」的根源：数组长度反映的是一页多少条，不是库里有多少条。
  const c = review.data?.counts;
  // mappings 不是本页的一档（审批在「数据映射」页），但计数照收：
  // 收件箱该说「有几条等你」
  const counts: Record<Sel | "mappings", number> = {
    overview: 0,
    pending: c?.pending ?? 0,
    duplicates: c?.duplicates ?? 0,
    conflicts: c?.conflicts ?? 0,
    unconfirmed: c?.unconfirmed ?? 0,
    lowconf: c?.lowconf ?? 0,
    mappings: c?.mappings ?? 0,
    violations: c?.violations ?? 0,
    defects: c?.defects ?? 0,
    alignment: c?.alignment ?? 0,
    merges: c?.merges ?? 0,
    decisions: history.data?.total ?? 0,
    agent: c?.agent ?? 0,
  };
  // 当前这一档的一页。**服务端已经切好了**，这里只按档收窄类型——
  // 收窄错了会在渲染时露馅，而不是悄悄显示空列表
  const rows = review.data?.queue === queueSel ? (review.data.items ?? []) : [];
  const asPending = () => rows as PendingFactItem[];
  const asDuplicates = () => rows as ReviewItem[];
  const asFacts = () => rows as FactReviewItem[];
  const asConflicts = () => rows as ConflictItem[];
  const asViolations = () => rows as AxiomViolation[];
  const asDefects = () => rows as OntologyDefect[];
  const asAlignment = () => rows as AlignmentItem[];
  // 对齐卡片要列本体的类与属性给人选；只在这一档拉
  const ontology = useQuery({
    queryKey: ["ontology", kb?.id],
    queryFn: () => api.ontology(kb!.id),
    enabled: !!kb && queueSel === "alignment",
  });
  const asMerges = () => rows as MergeLog[];
  const asAgent = () => rows as AgentDecision[];
  // agent 正在裁的一对（0025）：任务在跑、这一对标着 adjudicating。批量选页时跳过它们
  const lockedByAgent = (item: ReviewItem) =>
    !!c?.agent_running && item.stage === "adjudicating";
  const selectable = () => asDuplicates().filter((d) => !lockedByAgent(d));
  const queueEmpty = QUEUE_ORDER.every((k) => counts[k] === 0);

  // 没带 ?queue= 进来就落在总览上——从前是「第一个非空队列」，那等于替人
  // 决定先看哪一档；现在先给全貌，哪一档先办由人挑
  const select = (s: Sel) => {
    setSel(s);
    setPage(0);
  };

  const active: Sel = sel ?? "overview";
  const isQueueSel = QUEUE_ORDER.includes(active);

  const SECTION: Record<Sel, { title: string; hint: string | null }> = {
    overview: { title: S.review.overviewTitle, hint: S.review.overviewHint },
    pending: { title: S.review.pending, hint: S.review.pendingHint },
    duplicates: { title: S.review.duplicates, hint: S.review.duplicatesHint },
    conflicts: { title: S.review.conflicts, hint: S.review.conflictsHint },
    unconfirmed: {
      title: S.review.unconfirmed,
      hint: S.review.unconfirmedHint,
    },
    lowconf: {
      title: S.review.lowConfidence,
      hint: S.review.lowConfidenceHint,
    },
    violations: {
      title: S.review.violations,
      hint: S.review.violationsHint,
    },
    defects: { title: S.review.defects, hint: S.review.defectsHint },
    alignment: { title: S.review.alignment, hint: S.review.alignmentHint },
    decisions: { title: S.review.decisionsTitle, hint: S.review.decisionsHint },
    merges: { title: S.review.mergeHistory, hint: null },
    agent: { title: S.review.agentTitle, hint: S.review.agentHint },
  };

  return (
    <div className="h-full flex">
      {/* 左栏：队列分类 + 历史，各带实时计数（SSE 推动刷新） */}
      {/* `overflow-y-auto`：矮窗口下这一栏的内容比它高，而底部那条是「去别处办」
          的出口——没有滚动它会被裁掉且够不着。`mt-auto` 只在有富余空间时把它
          压到底，两者要一起给 */}
      <aside className={`${RAIL_CLS} flex flex-col overflow-y-auto u-scroll`}>
        {/* 总览在最上面，七档队列直接排在它下面，不另起标题——「队列」这个词
            说的是它们是什么，而人要的是它们有多少 */}
        <div className="u-rail-list px-2 pt-3">
          <RailItem
            active={active === "overview"}
            icon={<LayoutDashboard size={14} />}
            onClick={() => select("overview")}
          >
            {S.review.railOverview}
          </RailItem>
        </div>
        <div className="u-rail-list px-2 pt-1">
          <RailItem
            active={active === "pending"}
            count={counts.pending}
            onClick={() => select("pending")}
          >
            {S.review.railPending}
          </RailItem>
          <RailItem
            active={active === "duplicates"}
            count={counts.duplicates}
            onClick={() => select("duplicates")}
          >
            {S.review.railDuplicates}
          </RailItem>
          <RailItem
            active={active === "conflicts"}
            count={counts.conflicts}
            onClick={() => select("conflicts")}
          >
            {S.review.railConflicts}
          </RailItem>
          <RailItem
            active={active === "unconfirmed"}
            count={counts.unconfirmed}
            onClick={() => select("unconfirmed")}
          >
            {S.review.railUnconfirmed}
          </RailItem>
          <RailItem
            active={active === "lowconf"}
            count={counts.lowconf}
            onClick={() => select("lowconf")}
          >
            {S.review.railLowConfidence}
          </RailItem>
          <RailItem
            active={active === "violations"}
            count={counts.violations}
            onClick={() => select("violations")}
          >
            {S.review.railViolations}
          </RailItem>
          <RailItem
            active={active === "defects"}
            count={counts.defects}
            onClick={() => select("defects")}
          >
            {S.review.railDefects}
          </RailItem>
          <RailItem
            active={active === "alignment"}
            count={counts.alignment}
            onClick={() => select("alignment")}
          >
            {S.review.railAlignment}
          </RailItem>
          {/* agent 的队列（0025）：徽标是等人回答的建议数 */}
          <RailItem
            active={active === "agent"}
            count={counts.agent}
            dot={c?.agent_running ? "bg-warn animate-pulse" : undefined}
            onClick={() => select("agent")}
          >
            {S.review.railAgent}
          </RailItem>
        </div>
        <RailHeader label={S.review.tabHistory} />
        <div className="u-rail-list px-2">
          <RailItem
            active={active === "decisions"}
                        onClick={() => select("decisions")}
          >
            {S.review.railDecisions}
          </RailItem>
          <RailItem
            active={active === "merges"}
            count={counts.merges}
            onClick={() => select("merges")}
          >
            {S.review.railMerges}
          </RailItem>
        </div>

        {/* 数据映射：**两组都不属于，所以压在底部单独一条。**
            上面那七档问的都是「这条知识对不对」，而口径问的是「这个数怎么算」
            （0011 已经在数据层把它分出去了）；下面那两档是本页办过的事的流水，
            而口径的决定从来不进 `review_history`（它只捞 review./fact./
            conflict./merge.，口径记的是 mapping.decided）。
            **计数留着**——收件箱该说「有几条等你」，但活在有上下文的那一页干 */}
        <div className="mt-auto border-t border-line px-2 py-2">
          <RailItem
            active={false}
            count={counts.mappings}
            onClick={() =>
              navigate({ to: "/kb/$kbId/mappings", params: { kbId } })
            }
            external
          >
            {S.review.railMappings}
          </RailItem>
        </div>
      </aside>

      {/* 右侧：一次只显示选中的一类，单一分页 */}
      <div className="flex-1 min-w-0 overflow-y-auto u-scroll px-8 py-6">
        <div>
          {review.isPending && (
            <p className="text-body text-ink-2">{S.nav.loading}</p>
          )}
          {review.isError && (
            <p className="text-body text-danger">
              {(review.error as Error).message}
            </p>
          )}

          {review.data && (
            <section>
              <PageHeader title={SECTION[active].title} sub={SECTION[active].hint} />

              {/* 空态：整个待办全清 vs 单类清空。**公理这一档除外**——它自己那句要
                  分清「查过、没矛盾」和「还没查过」，通用空态说不出这个差别 */}
              {isQueueSel &&
                active !== "violations" &&
                active !== "defects" &&
                counts[active] === 0 && (
                  <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
                    {queueEmpty ? S.review.empty : S.review.categoryEmpty}
                  </div>
                )}

              {active === "overview" &&
                (summary.isPending ? (
                  <p className="text-body text-ink-2">{S.nav.loading}</p>
                ) : summary.isError ? (
                  <p className="text-body text-danger">
                    {(summary.error as Error).message}
                  </p>
                ) : summary.data ? (
                  <ReviewOverview
                    summary={summary.data}
                    governance={kb?.governance ?? false}
                    onPick={select}
                    onSettings={() =>
                      navigate({ to: "/kb/$kbId/settings", params: { kbId } })
                    }
                  />
                ) : null)}

              {active === "duplicates" && counts.duplicates > 0 && (
                <div className="space-y-3">
                  {/* 按两边类型的关系筛（#428）：同名同类那一档是人最先想一把
                      合掉的，类型冲突那一档是绝不能合的；三档各带真实条数 */}
                  <div className="flex flex-wrap items-center gap-3">
                    <Segmented
                      size="sm"
                      value={types}
                      onChange={(v) => {
                        setTypes(v);
                        setPage(0);
                      }}
                      options={[
                        { value: "any", label: S.review.typesAny, count: counts.duplicates },
                        {
                          value: "same",
                          label: S.review.typesSame,
                          count: c?.duplicates_same_type ?? 0,
                        },
                        {
                          value: "conflict",
                          label: S.review.typesConflict,
                          count: c?.duplicates_type_conflict ?? 0,
                        },
                      ]}
                    />
                    {/* 批量：勾选这一页的，一个动作裁一批。按钮只在有选中时出现 */}
                    <Checkbox
                      className="ml-auto"
                      checked={
                        selectable().length > 0 &&
                        selectable().every((d) => picked.has(d.id))
                      }
                      onChange={(v) =>
                        setPicked(
                          v
                            ? new Set(selectable().map((d) => d.id))
                            : new Set(),
                        )
                      }
                      label={S.review.selectPage}
                    />
                    {picked.size > 0 && (
                      <>
                        <span className="u-num text-small text-ink-2">
                          {S.review.selected(picked.size)}
                        </span>
                        {/* 一批一句理由（0026）：这一批为什么一起这么定 */}
                        <Input
                          size="sm"
                          className="w-44"
                          placeholder={S.review.rationalePlaceholder}
                          value={batchWhy}
                          disabled={batch.isPending}
                          onChange={(e) => setBatchWhy(e.target.value)}
                        />
                        <Button
                          variant="secondary"
                          size="sm"
                          disabled={batch.isPending}
                          onClick={() =>
                            batch.mutate({
                              ids: [...picked],
                              action: "keep",
                              rationale: batchWhy,
                            })
                          }
                        >
                          {S.review.keepSelected}
                        </Button>
                        <Button
                          variant="primary"
                          size="sm"
                          disabled={batch.isPending}
                          onClick={() =>
                            batch.mutate({
                              ids: [...picked],
                              action: "merge",
                              rationale: batchWhy,
                            })
                          }
                        >
                          {S.review.mergeSelected}
                        </Button>
                      </>
                    )}
                  </div>
                  {asDuplicates().length === 0 && (
                    <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
                      {S.review.typesEmpty}
                    </div>
                  )}
                  {asDuplicates().map((item) => (
                    <DuplicateCard
                      key={item.id}
                      item={item}
                      locked={lockedByAgent(item)}
                      picked={picked.has(item.id)}
                      onPick={(on) =>
                        setPicked((prev) => {
                          const next = new Set(prev);
                          if (on) next.add(item.id);
                          else next.delete(item.id);
                          return next;
                        })
                      }
                      busy={
                        (decide.isPending && decide.variables?.id === item.id) ||
                        (batch.isPending && picked.has(item.id))
                      }
                      onDecide={(action, rationale) =>
                        decide.mutate({ id: item.id, action, rationale })
                      }
                    />
                  ))}
                </div>
              )}

              {active === "conflicts" && counts.conflicts > 0 && (
                <div className="space-y-3">
                  {asConflicts().map((c) => (
                    <ConflictRow
                      key={c.id}
                      conflict={c}
                      busy={
                        conflictAction.isPending &&
                        conflictAction.variables?.id === c.id
                      }
                      onResolve={(action, closeAt, closeAtPrecision) =>
                        conflictAction.mutate({
                          id: c.id,
                          action,
                          closeAt,
                          closeAtPrecision,
                        })
                      }
                    />
                  ))}
                </div>
              )}

              {active === "unconfirmed" && counts.unconfirmed > 0 && (
                <div className="space-y-3">
                  {asFacts().map((fact) => (
                    <UnconfirmedRow
                      key={fact.id}
                      fact={fact}
                      busy={
                        (factAction.isPending &&
                          factAction.variables?.id === fact.id) ||
                        (closeFactAction.isPending &&
                          closeFactAction.variables?.id === fact.id)
                      }
                      onReject={() =>
                        factAction.mutate({ id: fact.id, action: "reject" })
                      }
                      onClose={(validTo, precision) =>
                        closeFactAction.mutate({ id: fact.id, validTo, precision })
                      }
                    />
                  ))}
                </div>
              )}

              {active === "pending" && counts.pending > 0 && (
                <div className="space-y-3">
                  {asPending().map((fact) => (
                    <PendingFactRow
                      key={fact.id}
                      fact={fact}
                      canDecide={canDecidePending}
                      busy={
                        pendingAction.isPending &&
                        pendingAction.variables?.id === fact.id
                      }
                      onConfirm={() =>
                        pendingAction.mutate({ id: fact.id, action: "confirm" })
                      }
                      onReject={() =>
                        pendingAction.mutate({ id: fact.id, action: "reject" })
                      }
                    />
                  ))}
                </div>
              )}

              {active === "lowconf" && counts.lowconf > 0 && (
                <div className="space-y-3">
                  {asFacts().map((fact) => (
                    <FactRow
                      key={fact.id}
                      fact={fact}
                      busy={
                        factAction.isPending &&
                        factAction.variables?.id === fact.id
                      }
                      onConfirm={() =>
                        factAction.mutate({ id: fact.id, action: "confirm" })
                      }
                      onReject={() =>
                        factAction.mutate({ id: fact.id, action: "reject" })
                      }
                    />
                  ))}
                </div>
              )}

              {active === "defects" && (
                <div className="space-y-3">
                  {counts.defects === 0 && (
                    <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
                      {S.review.categoryEmpty}
                    </div>
                  )}
                  {asDefects().map((d) => (
                    <DefectRow
                      key={d.id}
                      defect={d}
                      busy={
                        defectAction.isPending &&
                        defectAction.variables?.id === d.id
                      }
                      onDecide={(resolution) =>
                        defectAction.mutate({ id: d.id, resolution })
                      }
                    />
                  ))}
                </div>
              )}

              {active === "alignment" && (
                <div className="space-y-3">
                  {counts.alignment === 0 && (
                    <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
                      {S.review.categoryEmpty}
                    </div>
                  )}
                  {asAlignment().map((item) =>
                    item.kind === "phrase" ? (
                      <AlignmentPhraseRow
                        key={item.id}
                        item={item}
                        properties={ontology.data?.relation_types ?? []}
                        busy={
                          alignmentPhraseAction.isPending &&
                          alignmentPhraseAction.variables?.id === item.id
                        }
                        onDecide={(property, direction) =>
                          alignmentPhraseAction.mutate({ id: item.id, property, direction })
                        }
                      />
                    ) : (
                      <AlignmentKindWordRow
                        key={item.kind_word}
                        item={item}
                        classes={ontology.data?.entity_types ?? []}
                        busy={
                          alignmentKindWordAction.isPending &&
                          alignmentKindWordAction.variables?.kindWord === item.kind_word
                        }
                        onDecide={(cls) =>
                          alignmentKindWordAction.mutate({ kindWord: item.kind_word, cls })
                        }
                      />
                    ),
                  )}
                </div>
              )}

              {active === "violations" && (
                <div className="space-y-3">
                  {/* 按钮在这一档里，不在页头：只有看这一档的人才想重跑。
                      报告留在按钮旁边——空结果要说清是「没矛盾」还是「没判据」 */}
                  <div className="flex items-center gap-3">
                    {/* ghost 而不是实心白：这和「探查映射」是同一种东西——
                        手动触发一次分析，不是这一页的主操作。留一个实心白给
                        真正的决定（确认 / 合并） */}
                    <Button variant="secondary" size="sm"
                      disabled={runCheck.isPending}
                      onClick={() => runCheck.mutate()}
                    >
                      {runCheck.isPending
                        ? S.review.checking
                        : S.review.runCheck}
                    </Button>
                    {runCheck.data && (
                      <span className="text-small text-ink-2">
                        {/* 三种结果说三句话。**`found` 不是要报的数**：
                            重跑会把已裁决的那些重新算出来，说「3 处矛盾」而
                            列表只剩一条，看起来像界面漏了东西 */}
                        {runCheck.data.predicates_with_axioms === 0
                          ? S.review.checkNoAxioms
                          : runCheck.data.inserted > 0
                            ? S.review.checkFound(runCheck.data.inserted)
                            : runCheck.data.found > 0
                              ? S.review.checkNothingNew
                              : S.review.checkClean(runCheck.data.edges)}
                      </span>
                    )}
                  </div>
                  {counts.violations === 0 && !runCheck.data && (
                    <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
                      {S.review.checkNeverRun}
                    </div>
                  )}
                  {asViolations().map((v) => (
                    <div
                      key={v.id}
                      className={
                        v.id === search.item
                          ? "rounded-panel ring-1 ring-contest"
                          : undefined
                      }
                    >
                    <ViolationRow
                      violation={v}
                      busy={
                        violationAction.isPending &&
                        violationAction.variables?.id === v.id
                      }
                      onDecide={(resolution, opts) =>
                        violationAction.mutate({ id: v.id, resolution, opts })
                      }
                      onDuplicates={() => select("duplicates")}
                      onOntology={() => navigate({ to: "/ontology" })}
                    />
                    </div>
                  ))}
                </div>
              )}

              {active === "agent" &&
                ((c?.agent_rows ?? 0) === 0 ? (
                  <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
                    {S.review.agentEmpty}
                    {!kb?.governance && (
                      <>
                        {" "}
                        {S.review.overviewAgentOff}{" "}
                        <LinkButton
                          onClick={() =>
                            navigate({ to: "/kb/$kbId/settings", params: { kbId } })
                          }
                        >
                          {S.review.overviewAgentSettings}
                        </LinkButton>
                      </>
                    )}
                  </div>
                ) : (
                  <div className="space-y-2">
                    {asAgent().map((d) => (
                      <AgentRow
                        key={d.id}
                        d={d}
                        busy={agentAnswer.isPending && agentAnswer.variables?.id === d.id}
                        onAnswer={(action, rationale) =>
                          agentAnswer.mutate({ id: d.id, action, rationale })
                        }
                      />
                    ))}
                  </div>
                ))}

              {active === "merges" &&
                (counts.merges === 0 ? (
                  <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
                    {S.review.historyEmpty}
                  </div>
                ) : (
                  <div className="space-y-2">
                    {asMerges().map((m) => (
                      <MergeRow
                        key={m.id}
                        merge={m}
                        busy={revert.isPending && revert.variables === m.id}
                        onRevert={() => revert.mutate(m.id)}
                      />
                    ))}
                  </div>
                ))}

              {active === "decisions" &&
                (history.isPending ? (
                  <p className="text-body text-ink-2">{S.nav.loading}</p>
                ) : (history.data?.total ?? 0) === 0 ? (
                  <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
                    {S.review.decisionsEmpty}
                  </div>
                ) : (
                  <div className="space-y-2">
                    {(history.data?.events ?? []).map((e) => (
                      <DecisionRow key={e.id} e={e} />
                    ))}
                  </div>
                ))}

              {/* 单一分页：queue/merges 走客户端切片，decisions 服务端分页；总览没有页 */}
              {active !== "decisions" && active !== "overview" && (
                <Pager
                  total={active === "agent" ? (c?.agent_rows ?? 0) : counts[active]}
                  pageSize={PAGE_SIZE[active]}
                  page={page}
                  onPage={setPage}
                />
              )}
              {active === "decisions" && (
                <Pager
                  total={history.data?.total ?? 0}
                  pageSize={PAGE_SIZE.decisions}
                  page={page}
                  onPage={setPage}
                />
              )}
            </section>
          )}
        </div>
      </div>
    </div>
  );
}
