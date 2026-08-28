/* 实体的认知变更历史（记录时间轴）。
   与同面板的 Timeline 视图正交：那条轴问"这件事在现实里何时成立"，这条轴问
   "我们何时这么认为、又何时改了主意"。数据来自 append-only 账本里那些被
   entity_detail 用 invalidated_at IS NULL 滤掉的行。 */
import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { FileText, Merge, PencilLine, Undo2 } from "lucide-react";
import { api, type EntityHistoryEvent } from "../api";
import { S } from "../i18n";
import { Pager } from "../ui";

const PER = 20;

/** 事件类型 → 图标与色调（语义色只给"被推翻"，其余保持中性） */
const KIND_ICON = {
  asserted: FileText,
  corrected: PencilLine,
  rejected: Undo2,
  // 并入不是撤回：内容一字未少地进了另一条断言
  merged: Merge,
} as const;

const KIND_TONE: Record<string, string> = {
  asserted: "text-neutral-500",
  corrected: "text-[var(--u-warn)]",
  rejected: "text-[var(--u-danger)]",
  merged: "text-neutral-500",
};

const ymd = (iso: string) => iso.slice(0, 10);
const ym = (iso: string | null) => (iso ? iso.slice(0, 7) : null);

/** 宾语：实体名优先，其次字面值（属性事实）的摘要/值 */
function objectText(e: EntityHistoryEvent): string {
  if (e.other_name) return e.other_name;
  const v = e.object_value as { summary?: unknown; value?: unknown } | null;
  const raw = v?.summary ?? v?.value;
  return raw === undefined || raw === null ? "—" : String(raw);
}

/** 这次变更对有效区间做了什么（记录轴上的事件，改的是有效轴上的边界） */
function intervalNote(e: EntityHistoryEvent): string | null {
  if (e.kind === "corrected") {
    return e.valid_to ? S.graph.historyClosedAt(ym(e.valid_to)!) : null;
  }
  const from = ym(e.valid_from);
  if (!from) return null;
  return e.valid_to
    ? `${from} → ${ym(e.valid_to)}`
    : `${S.graph.historyFrom(from)} · ${S.graph.historyOngoing}`;
}

function EventRow({ e }: { e: EntityHistoryEvent }) {
  const Icon = KIND_ICON[e.kind] ?? FileText;
  const note = intervalNote(e);
  return (
    <div className="flex gap-2.5 px-2 py-2">
      <Icon size={13} className={`mt-0.5 shrink-0 ${KIND_TONE[e.kind] ?? ""}`} />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-1.5 flex-wrap">
          <span className="text-[11px] font-medium text-neutral-300">
            {S.graph.historyKind[e.kind] ?? e.kind}
          </span>
          {note && <span className="u-num text-[11px] text-neutral-500">{note}</span>}
        </div>
        <div className="mt-0.5 text-[12.5px] text-neutral-400 truncate">
          <span className="text-neutral-500">
            {e.direction === "in" ? "← " : ""}
            {e.predicate_label}
            {e.direction === "in" ? "" : " →"}
          </span>{" "}
          <span className="text-neutral-200">{objectText(e)}</span>
        </div>
        <div className="mt-0.5 flex items-center gap-1.5 text-[10.5px] text-neutral-600">
          <span className="u-num">{ymd(e.at)}</span>
          <span>·</span>
          {/* 归因：人名，或引擎（抽取写入 / 时态对账自动闭合） */}
          <span>{e.actor_name ?? S.graph.historyEngine}</span>
          {e.filename && e.document_id && (
            <>
              <span>·</span>
              <Link
                to="/doc/$docId"
                params={{ docId: e.document_id }}
                search={{}}
                className="truncate hover:text-neutral-300"
                title={e.quote ?? e.filename}
              >
                {e.filename}
              </Link>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

export function EntityHistory({ kbId, entityId }: { kbId: string; entityId: string }) {
  const [page, setPage] = useState(0);
  useEffect(() => setPage(0), [entityId]);
  const q = useQuery({
    queryKey: ["entityHistory", kbId, entityId, page],
    queryFn: () => api.entityHistory(kbId, entityId, page, PER),
  });

  const total = q.data?.total ?? 0;
  if (q.isPending) return <p className="p-2 text-sm text-neutral-500">{S.nav.loading}</p>;
  // 只有"一条都没有"才是空。记录轴上首次断言本身就是一次事件——
  // "我们何时、从哪份文档得知这件事"是这条轴要回答的问题的一半
  if (total === 0) return <p className="p-2 text-xs text-neutral-500">{S.graph.historyEmpty}</p>;

  return (
    <div>
      <p className="px-2 pb-1.5 text-[11px] text-neutral-600">{S.graph.historyHint}</p>
      <div className="divide-y divide-white/[0.06]">
        {(q.data?.events ?? []).map((e) => (
          <EventRow key={`${e.fact_id}-${e.kind}`} e={e} />
        ))}
      </div>
      <Pager total={total} pageSize={PER} page={page} onPage={setPage} />
    </div>
  );
}
