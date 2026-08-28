import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  api,
  type ConflictItem,
  type FactReviewItem,
  type MergeLog,
  type ReviewHistoryEvent,
  type ReviewItem,
  type ReviewSide,
} from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { Chip, type ChipTone, Pager, RAIL_CLS, pageSlice, cn } from "../ui";

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
  return detail ? S.errDetail(worded, detail) : worded;
}

function dateRange(from: string | null, to: string | null): string | null {
  if (!from && !to) return null;
  return `${ym(from) ?? "…"} → ${ym(to) ?? S.review.ongoing}`;
}

function SideCard({ side }: { side: ReviewSide }) {
  return (
    <div className="flex-1 min-w-0">
      <div className="flex items-center gap-2 mb-1">
        <span
          className="h-2.5 w-2.5 rounded-full shrink-0"
          style={{ backgroundColor: side.color }}
        />
        <span className="text-sm font-medium text-white truncate">
          {side.name}
        </span>
        {side.disambiguator && (
          <span className="text-xs text-neutral-500 truncate">
            · {side.disambiguator}
          </span>
        )}
      </div>
      <div className="text-xs text-neutral-500 mb-2">
        {side.type_label} · {S.review.factsCount(side.degree)}
      </div>
      {side.top_facts.length > 0 ? (
        <ul className="space-y-1">
          {side.top_facts.map((f, i) => (
            <li key={i} className="text-xs text-neutral-400 truncate">
              {f}
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-xs text-neutral-600">{S.review.noFacts}</p>
      )}
    </div>
  );
}

function DuplicateCard({
  item,
  busy,
  onDecide,
}: {
  item: ReviewItem;
  busy: boolean;
  onDecide: (action: "merge" | "keep") => void;
}) {
  return (
    <div className="glass rounded-xl p-4">
      <div className="flex gap-4">
        <SideCard side={item.left} />
        <div className="self-center text-neutral-600 text-sm shrink-0">≟</div>
        <SideCard side={item.right} />
      </div>
      <div className="mt-3 pt-3 flex items-center gap-3 border-t border-[var(--u-line)]">
        <span
          className={`u-chip ${item.stage === "human" ? "u-chip-warn" : "u-chip-neutral"}`}
        >
          {item.stage === "human"
            ? S.review.stageHuman
            : S.review.stageAdjudicating}
        </span>
        <span className="text-xs text-neutral-500">
          {S.review.similarity(Math.round(item.score * 100))}
        </span>
        {item.reason && (
          <span className="text-xs text-neutral-600 truncate min-w-0">
            {escalationText(item.reason)}
          </span>
        )}
        <div className="ml-auto flex gap-2 shrink-0">
          <button
            className="u-btn u-btn-ghost px-3 py-1.5 text-xs"
            disabled={busy}
            onClick={() => onDecide("keep")}
          >
            {S.review.keep}
          </button>
          <button
            className="u-btn u-btn-primary px-3 py-1.5 text-xs"
            disabled={busy}
            onClick={() => onDecide("merge")}
          >
            {S.review.merge}
          </button>
        </div>
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
    <div className="glass rounded-xl p-4">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-sm font-medium text-white">
          {fact.subject_name}
        </span>
        <span className="text-xs text-neutral-500">
          — {fact.predicate_label} →
        </span>
        <span className="text-sm font-medium text-white">
          {fact.object_name ?? "?"}
        </span>
        {range && <span className="text-xs text-neutral-500">({range})</span>}
        <span className="u-chip u-chip-warn ml-auto">
          {S.review.confidence(Math.round(fact.confidence * 100))}
        </span>
      </div>
      {fact.quote && (
        <p className="mt-2 text-xs text-neutral-500 italic line-clamp-2">
          “{fact.quote}”
        </p>
      )}
      <div className="mt-3 flex gap-2 justify-end">
        <button
          className="u-btn u-btn-ghost px-3 py-1.5 text-xs text-[var(--u-danger)]"
          disabled={busy}
          onClick={onReject}
        >
          {S.review.reject}
        </button>
        <button
          className="u-btn u-btn-ghost px-3 py-1.5 text-xs"
          disabled={busy}
          onClick={onConfirm}
        >
          {S.review.confirm}
        </button>
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
  ) => void;
}) {
  const [closeAt, setCloseAt] = useState("");
  const c = conflict;
  const needsDate = !c.new_valid_from;
  // close_at 输入按天精度转 RFC3339
  const closeAtIso = /^\d{4}-\d{2}-\d{2}$/.test(closeAt.trim())
    ? `${closeAt.trim()}T00:00:00Z`
    : undefined;

  return (
    <div className="glass rounded-xl p-4">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-sm font-medium text-white">{c.old_subject}</span>
        <span className="text-xs text-neutral-500">
          — {c.predicate_label} →
        </span>
        <span className="text-sm font-medium text-white">
          {c.old_object ?? "?"}
        </span>
        {c.old_valid_from && (
          <span className="u-num text-xs text-neutral-500">
            ({S.review.conflictSince(c.old_valid_from.slice(0, 10))})
          </span>
        )}
        <span className="text-xs text-neutral-600">{S.review.conflictVs}</span>
        <span className="text-sm font-medium text-white">{c.new_subject}</span>
        <span className="text-xs text-neutral-500">
          — {c.predicate_label} →
        </span>
        <span className="text-sm font-medium text-white">
          {c.new_object ?? "?"}
        </span>
        {c.new_valid_from && (
          <span className="u-num text-xs text-neutral-500">
            ({S.review.conflictSince(c.new_valid_from.slice(0, 10))})
          </span>
        )}
        <span className="u-chip u-chip-warn ml-auto">
          {S.review.conflictReason[c.reason] ?? c.reason}
        </span>
      </div>
      <div className="mt-3 flex items-center gap-2 justify-end">
        <button
          className="u-btn u-btn-ghost px-3 py-1.5 text-xs text-[var(--u-danger)]"
          disabled={busy}
          onClick={() => onResolve("reject_new")}
        >
          {S.review.rejectNew}
        </button>
        <button
          className="u-btn u-btn-ghost px-3 py-1.5 text-xs"
          disabled={busy}
          onClick={() => onResolve("keep")}
        >
          {S.review.keepBoth}
        </button>
        {needsDate && (
          <input
            className="input-dark u-num w-28 px-2 py-1.5 text-xs text-center"
            placeholder={S.review.closeAtPlaceholder}
            value={closeAt}
            onChange={(e) => setCloseAt(e.target.value)}
          />
        )}
        <button
          className="u-btn u-btn-ghost px-3 py-1.5 text-xs"
          disabled={busy || (needsDate && !closeAtIso)}
          onClick={() => onResolve("close", closeAtIso)}
        >
          {c.new_valid_from
            ? S.review.closeOldAt(c.new_valid_from.slice(0, 10))
            : S.review.closeOld}
        </button>
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
  onClose: (validTo: string) => void;
}) {
  const [closeAt, setCloseAt] = useState("");
  const closeAtIso = /^\d{4}-\d{2}-\d{2}$/.test(closeAt.trim())
    ? `${closeAt.trim()}T00:00:00Z`
    : undefined;
  const range = dateRange(fact.valid_from, fact.valid_to);

  return (
    <div className="glass rounded-xl p-4">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-sm font-medium text-white">
          {fact.subject_name}
        </span>
        <span className="text-xs text-neutral-500">
          — {fact.predicate_label} →
        </span>
        <span className="text-sm font-medium text-white">
          {fact.object_name ?? "?"}
        </span>
        {range && (
          <span className="u-num text-xs text-neutral-500">({range})</span>
        )}
      </div>
      {fact.quote && (
        <p className="mt-2 text-xs text-neutral-500 italic line-clamp-2">
          “{fact.quote}”
        </p>
      )}
      <div className="mt-3 flex items-center gap-2 justify-end">
        <button
          className="u-btn u-btn-ghost px-3 py-1.5 text-xs text-[var(--u-danger)]"
          disabled={busy}
          onClick={onReject}
        >
          {S.review.reject}
        </button>
        <input
          className="input-dark u-num w-28 px-2 py-1.5 text-xs text-center"
          placeholder={S.review.closeAtPlaceholder}
          value={closeAt}
          onChange={(e) => setCloseAt(e.target.value)}
        />
        <button
          className="u-btn u-btn-ghost px-3 py-1.5 text-xs"
          disabled={busy || !closeAtIso}
          onClick={() => closeAtIso && onClose(closeAtIso)}
        >
          {closeAt.trim()
            ? S.review.closeFactAt(closeAt.trim())
            : S.review.closeFact}
        </button>
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
    <div className="glass rounded-xl px-4 py-3 flex items-center gap-3">
      <div className="min-w-0 flex-1">
        <div className="text-sm text-neutral-300 truncate">
          <span className="text-neutral-500">{merge.source_name}</span>
          <span className="text-neutral-600"> → </span>
          <span className="text-white">{merge.target_name}</span>
        </div>
        <div className="text-xs text-neutral-500 truncate">
          {merge.merged_by_name
            ? S.review.mergedBy(merge.merged_by_name)
            : S.review.mergedByAi}
          {" · "}
          {merge.created_at.slice(0, 10)}
          {merge.reason ? ` · ${merge.reason}` : ""}
        </div>
      </div>
      {merge.reverted_at ? (
        <span className="u-chip u-chip-neutral shrink-0">
          {S.review.reverted}
        </span>
      ) : (
        <button
          className="u-btn u-btn-ghost px-3 py-1.5 text-xs shrink-0"
          disabled={busy}
          onClick={onRevert}
        >
          {S.review.revert}
        </button>
      )}
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
    text = `${d.subject} — ${d.predicate} → ${d.object ?? "?"}`;
  else if (e.action.startsWith("conflict."))
    text = `${d.old_subject} — ${d.predicate} → ${d.old_object ?? "?"} · vs · ${
      d.new_object ?? d.new_subject
    }`;
  else text = `${d.source} → ${d.target}`;

  return (
    <div className="glass rounded-xl px-4 py-3 flex items-center gap-3">
      <Chip tone={DECISION_TONE[e.action] ?? "neutral"}>
        {S.review.decisionActions[e.action] ?? e.action}
      </Chip>
      <span className="text-sm text-neutral-300 truncate min-w-0">{text}</span>
      {typeof d.confidence === "number" && (
        <span className="u-num text-xs text-neutral-600 shrink-0">
          {Math.round(d.confidence * 100)}%
        </span>
      )}
      {typeof d.valid_to === "string" && (
        <span className="u-num text-xs text-neutral-600 shrink-0">
          → {d.valid_to.slice(0, 10)}
        </span>
      )}
      <span className="ml-auto shrink-0 text-xs text-neutral-500">
        {e.actor_name ?? S.review.aiActor}
        {" · "}
        <span className="u-num">{e.created_at.slice(0, 10)}</span>
      </span>
    </div>
  );
}

/* ---------- 页面：左栏分类 + 单类内容区 ---------- */

type Sel =
  | "duplicates"
  | "conflicts"
  | "unconfirmed"
  | "lowconf"
  | "decisions"
  | "merges";

const QUEUE_ORDER: Sel[] = [
  "duplicates",
  "conflicts",
  "unconfirmed",
  "lowconf",
];
const PAGE_SIZE: Record<Sel, number> = {
  duplicates: DUP_PAGE,
  conflicts: CONFLICT_PAGE,
  unconfirmed: FACT_PAGE,
  lowconf: FACT_PAGE,
  merges: MERGE_PAGE,
  decisions: 20,
};

function RailHeader({ label }: { label: string }) {
  return (
    <div className="px-4 pt-4 pb-1.5 text-[10px] font-medium uppercase tracking-[0.08em] text-neutral-500">
      {label}
    </div>
  );
}

function RailItem({
  active,
  label,
  count,
  onClick,
}: {
  active: boolean;
  label: string;
  count: number | null;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "w-full flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-[13px] transition-colors",
        active
          ? "u-nav-active"
          : "text-neutral-400 hover:bg-white/[0.05] hover:text-neutral-200",
      )}
    >
      <span className="truncate">{label}</span>
      {count !== null && (
        <span
          className={cn(
            "ml-auto shrink-0 u-num text-[10.5px]",
            count > 0 ? "text-neutral-400" : "text-neutral-700",
          )}
        >
          {count}
        </span>
      )}
    </button>
  );
}

export function Review() {
  const { kb } = useKb();
  const queryClient = useQueryClient();
  const [sel, setSel] = useState<Sel | null>(null);
  const [page, setPage] = useState(0);

  // 队列变化经 SSE 事件流推送（useKbEvents 挂在 Shell），无需轮询
  const review = useQuery({
    queryKey: ["review", kb?.id],
    queryFn: () => api.review(kb!.id),
    enabled: !!kb,
  });
  // 决策台账：服务端分页，仅选中时拉取
  const history = useQuery({
    queryKey: ["reviewHistory", kb?.id, page],
    queryFn: () => api.reviewHistory(kb!.id, page),
    enabled: !!kb && sel === "decisions",
  });

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["review", kb?.id] });
    queryClient.invalidateQueries({ queryKey: ["reviewHistory", kb?.id] });
    queryClient.invalidateQueries({ queryKey: ["graph"] });
  };

  const decide = useMutation({
    mutationFn: ({ id, action }: { id: string; action: "merge" | "keep" }) =>
      api.decideReview(kb!.id, id, action),
    onSettled: invalidate,
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
  const revert = useMutation({
    mutationFn: (mergeId: string) => api.revertMerge(kb!.id, mergeId),
    onSettled: invalidate,
  });
  const conflictAction = useMutation({
    mutationFn: ({
      id,
      action,
      closeAt,
    }: {
      id: string;
      action: "close" | "keep" | "reject_new";
      closeAt?: string;
    }) => api.resolveConflict(kb!.id, id, { action, close_at: closeAt }),
    onSettled: invalidate,
  });

  const closeFactAction = useMutation({
    mutationFn: ({ id, validTo }: { id: string; validTo: string }) =>
      api.closeFact(kb!.id, id, validTo),
    onSettled: invalidate,
  });

  const data = review.data;
  const counts: Record<Sel, number> = {
    duplicates: data?.reviews.length ?? 0,
    conflicts: data?.conflicts?.length ?? 0,
    unconfirmed: data?.unconfirmed?.length ?? 0,
    lowconf: data?.facts.length ?? 0,
    merges: data?.merges.length ?? 0,
    decisions: history.data?.total ?? 0,
  };
  const queueEmpty = QUEUE_ORDER.every((k) => counts[k] === 0);

  // 首批数据到达：定位到第一个非空队列（全空落在 duplicates 显示"干净"文案）
  useEffect(() => {
    if (sel === null && data)
      setSel(QUEUE_ORDER.find((k) => counts[k] > 0) ?? "duplicates");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data]);

  const select = (s: Sel) => {
    setSel(s);
    setPage(0);
  };

  const active = sel ?? "duplicates";
  const isQueueSel = QUEUE_ORDER.includes(active);

  const SECTION: Record<Sel, { title: string; hint: string | null }> = {
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
    decisions: { title: S.review.decisionsTitle, hint: S.review.decisionsHint },
    merges: { title: S.review.mergeHistory, hint: null },
  };

  return (
    <div className="h-full flex">
      {/* 左栏：队列分类 + 历史，各带实时计数（SSE 推动刷新） */}
      <aside className={`${RAIL_CLS} flex flex-col`}>
        <RailHeader label={S.review.tabQueue} />
        <div className="px-2 space-y-0.5">
          <RailItem
            active={active === "duplicates"}
            label={S.review.railDuplicates}
            count={counts.duplicates}
            onClick={() => select("duplicates")}
          />
          <RailItem
            active={active === "conflicts"}
            label={S.review.railConflicts}
            count={counts.conflicts}
            onClick={() => select("conflicts")}
          />
          <RailItem
            active={active === "unconfirmed"}
            label={S.review.railUnconfirmed}
            count={counts.unconfirmed}
            onClick={() => select("unconfirmed")}
          />
          <RailItem
            active={active === "lowconf"}
            label={S.review.railLowConfidence}
            count={counts.lowconf}
            onClick={() => select("lowconf")}
          />
        </div>
        <RailHeader label={S.review.tabHistory} />
        <div className="px-2 space-y-0.5">
          <RailItem
            active={active === "decisions"}
            label={S.review.railDecisions}
            count={null}
            onClick={() => select("decisions")}
          />
          <RailItem
            active={active === "merges"}
            label={S.review.railMerges}
            count={counts.merges}
            onClick={() => select("merges")}
          />
        </div>
      </aside>

      {/* 右侧：一次只显示选中的一类，单一分页 */}
      <div className="flex-1 min-w-0 overflow-y-auto u-scroll px-8 py-6">
        <div className="max-w-4xl">
          {review.isPending && (
            <p className="text-sm text-neutral-500">{S.nav.loading}</p>
          )}
          {review.isError && (
            <p className="text-sm text-rose-400">
              {(review.error as Error).message}
            </p>
          )}

          {data && (
            <section>
              {/* 页级标题：与 Library/KB Settings 同级（text-lg），不是卡片头 */}
              <h2 className="u-title text-lg mb-1">{SECTION[active].title}</h2>
              {SECTION[active].hint && (
                <p className="text-xs text-neutral-500 mb-3">
                  {SECTION[active].hint}
                </p>
              )}

              {/* 空态：整个待办全清 vs 单类清空 */}
              {isQueueSel && counts[active] === 0 && (
                <div className="glass rounded-xl p-10 text-center text-sm text-neutral-500">
                  {queueEmpty ? S.review.empty : S.review.categoryEmpty}
                </div>
              )}

              {active === "duplicates" && counts.duplicates > 0 && (
                <div className="space-y-3">
                  {pageSlice(data.reviews, page, DUP_PAGE).rows.map((item) => (
                    <DuplicateCard
                      key={item.id}
                      item={item}
                      busy={
                        decide.isPending && decide.variables?.id === item.id
                      }
                      onDecide={(action) =>
                        decide.mutate({ id: item.id, action })
                      }
                    />
                  ))}
                </div>
              )}

              {active === "conflicts" && counts.conflicts > 0 && (
                <div className="space-y-3">
                  {pageSlice(data.conflicts, page, CONFLICT_PAGE).rows.map(
                    (c) => (
                      <ConflictRow
                        key={c.id}
                        conflict={c}
                        busy={
                          conflictAction.isPending &&
                          conflictAction.variables?.id === c.id
                        }
                        onResolve={(action, closeAt) =>
                          conflictAction.mutate({ id: c.id, action, closeAt })
                        }
                      />
                    ),
                  )}
                </div>
              )}

              {active === "unconfirmed" && counts.unconfirmed > 0 && (
                <div className="space-y-3">
                  {pageSlice(data.unconfirmed, page, FACT_PAGE).rows.map(
                    (fact) => (
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
                        onClose={(validTo) =>
                          closeFactAction.mutate({ id: fact.id, validTo })
                        }
                      />
                    ),
                  )}
                </div>
              )}

              {active === "lowconf" && counts.lowconf > 0 && (
                <div className="space-y-3">
                  {pageSlice(data.facts, page, FACT_PAGE).rows.map((fact) => (
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

              {active === "merges" &&
                (counts.merges === 0 ? (
                  <div className="glass rounded-xl p-10 text-center text-sm text-neutral-500">
                    {S.review.historyEmpty}
                  </div>
                ) : (
                  <div className="space-y-2">
                    {pageSlice(data.merges, page, MERGE_PAGE).rows.map((m) => (
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
                  <p className="text-sm text-neutral-500">{S.nav.loading}</p>
                ) : (history.data?.total ?? 0) === 0 ? (
                  <div className="glass rounded-xl p-10 text-center text-sm text-neutral-500">
                    {S.review.decisionsEmpty}
                  </div>
                ) : (
                  <div className="space-y-2">
                    {(history.data?.events ?? []).map((e) => (
                      <DecisionRow key={e.id} e={e} />
                    ))}
                  </div>
                ))}

              {/* 单一分页：queue/merges 走客户端切片，decisions 服务端分页 */}
              {active !== "decisions" && (
                <Pager
                  total={counts[active]}
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
