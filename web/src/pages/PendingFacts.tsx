/* 等人点头的事实（docs/decisions/0015）。
   一句 remember 抽出的三元组先进待确认队列，不上图；人在这里点头它才进账本。
   **原句在上，三元组在下**：只列三元组等于要人凭空判断它对不对——
   实测里 `Acme --?--> 深圳` 那条，人一看原句就知道该拒。
   两处共用同一行组件：Review 页的「待确认」一档，与 Chat 里跟在 remember 步骤后面的那张卡。 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type PendingFactItem } from "../api";
import { S } from "../i18n";
import { toast } from "../toast";

function ym(iso: string | null): string | null {
  return iso ? iso.slice(0, 7) : null;
}

/** 记忆落库时正文前面带着 `[YYYY-MM-DD HH:MM] `（`memory::append_episode` 加的时间戳）。
 *  卡片上要的是那句话本身，时间戳是索引用的，剥掉 */
function sentence(quote: string): string {
  return quote.replace(/^\[\d{4}-\d{2}-\d{2}(?: \d{2}:\d{2})?\]\s*/, "");
}

function objectText(f: PendingFactItem): string {
  if (f.object_name) return f.object_name;
  const v = f.object_value;
  if (!v) return "?";
  if (v.summary) return v.summary;
  const val = v.value === undefined || v.value === null ? "?" : String(v.value);
  return v.unit ? `${val} ${v.unit}` : val;
}

/** 点头是写图的动作，Editor 起步——与服务端 `require_kb(Role::Editor)` 同一口径。
 *  Viewer 看得见提议、看不见按钮：按钮亮着却点不动，等于让人猜自己有没有权限。
 *  查询键与 Library 页共用（`kbOne`），不多打一次接口 */
export function useCanDecide(kbId: string | undefined): boolean {
  const kbDetail = useQuery({
    queryKey: ["kbOne", kbId],
    queryFn: () => api.kbDetail(kbId!),
    enabled: !!kbId,
  });
  return ["editor", "admin", "owner"].includes(kbDetail.data?.my_role ?? "");
}

export function PendingFactRow({
  fact,
  busy,
  canDecide,
  onConfirm,
  onReject,
}: {
  fact: PendingFactItem;
  busy: boolean;
  canDecide: boolean;
  onConfirm: () => void;
  onReject: () => void;
}) {
  const from = ym(fact.valid_from);
  const to = ym(fact.valid_to);
  const range = from || to ? `${from ?? "…"} → ${to ?? S.review.ongoing}` : null;
  return (
    <div className="glass rounded-xl p-4">
      {/* 原句先出。它是人自己说的，判断的依据就是它 */}
      <p className="text-xs text-neutral-400 italic">“{sentence(fact.quote)}”</p>
      <div className="mt-2.5 flex items-center gap-2 flex-wrap">
        <span className="text-sm font-medium text-white">{fact.subject_name}</span>
        <span className="text-xs text-neutral-500">
          —{" "}
          {fact.predicate_label ? (
            <span>{fact.predicate_label}</span>
          ) : (
            /* 本体里没有这个关系：显示原话，斜体标明它不是词表里的词（0010） */
            <span
              className="italic text-neutral-600"
              title={S.review.pendingNoPredicate}
            >
              {fact.proposed_predicate ?? S.graph.unknownPredicate}
            </span>
          )}{" "}
          →
        </span>
        <span className="text-sm font-medium text-white">{objectText(fact)}</span>
        {range && <span className="text-xs text-neutral-500">({range})</span>}
        {!fact.predicate_label && (
          <span className="u-chip u-chip-warn ml-auto">{S.review.pendingNoPredicateChip}</span>
        )}
      </div>
      <div className="mt-3 flex items-center gap-2">
        {fact.proposed_by_name && (
          <span className="text-[11px] text-neutral-600">
            {fact.proposed_token_name
              ? S.review.pendingSaidVia(
                  fact.proposed_by_name,
                  fact.proposed_token_name,
                )
              : S.review.pendingSaidBy(fact.proposed_by_name)}
          </span>
        )}
        {canDecide && (
          <div className="ml-auto flex gap-2">
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
        )}
      </div>
    </div>
  );
}

/** 跟在 remember 步骤后面的确认卡。
 *  抽取是异步的，卡片在任务完成时才长出来（SSE `pending` 事件让查询失效重取）；
 *  回放旧会话时同样按 chunk 取——还有没点头的就照样显示，都处理完了就不占地方。 */
export function NodCard({ kbId, chunkId }: { kbId: string; chunkId: string }) {
  const queryClient = useQueryClient();
  const canDecide = useCanDecide(kbId);
  const q = useQuery({
    queryKey: ["pending", kbId, chunkId],
    queryFn: () => api.pendingForChunk(kbId, chunkId),
  });
  const decide = useMutation({
    mutationFn: ({ id, action }: { id: string; action: "confirm" | "reject" }) =>
      api.decidePending(kbId, id, action),
    onError: (e: Error) => toast.error(e.message),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["pending", kbId] });
      queryClient.invalidateQueries({ queryKey: ["review", kbId] });
    },
  });
  const items = q.data?.items ?? [];
  if (items.length === 0) return null;
  return (
    <div className="my-2 space-y-2">
      <div className="text-xs text-neutral-500">{S.review.nodCardTitle(items.length)}</div>
      {items.map((f) => (
        <PendingFactRow
          key={f.id}
          fact={f}
          busy={decide.isPending && decide.variables?.id === f.id}
          canDecide={canDecide}
          onConfirm={() => decide.mutate({ id: f.id, action: "confirm" })}
          onReject={() => decide.mutate({ id: f.id, action: "reject" })}
        />
      ))}
    </div>
  );
}
