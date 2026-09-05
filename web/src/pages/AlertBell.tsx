// 顶栏告警（0005）：铃铛 + 未读角标 + 弹出面板。
//
// **弹窗不是页面**：告警是"顺手瞄一眼"的东西，不是一个要专门去逛的地方。
// 做成页面会逼人离开手头的事，而离开的代价就是没人去看。
//
// 一条告警 = 一次故障，写完不再变，没有"已解决"。
// 「已读」逐人——一个人读过不代表别人也该从未读里消失。
import { type Ref, useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Bell, Search, X } from "lucide-react";

import { api, type AlertGroup } from "../api";
import { S } from "../i18n";
import { toast } from "../toast";
import {
  Button,
  Chip,
  cn,
  IconButton,
  Input,
  LinkButton,
  Pager,
} from "../ui";
import { usePopoverFlip } from "../ui/popoverFlip";

const PAGE = 8;

/** 明细里给人看的那一行：对象名 — 报错原文 */
function line(d: AlertGroup["lines"][number]): string | null {
  const parts = [d.name ?? d.job, d.error].filter(Boolean);
  return parts.length ? parts.join(" — ") : null;
}

/** 哪些告警带「再跑一遍」：故障修好之后（充值、改端点）任务不会自己回来的那几种 */
const REQUEUE_KINDS = new Set(["llm.out_of_credit", "llm.unreachable"]);

function AlertRow({
  g,
  onRead,
  onRequeue,
  requeuing,
}: {
  g: AlertGroup;
  onRead: (g: AlertGroup) => void;
  onRequeue: (g: AlertGroup) => void;
  requeuing: boolean;
}) {
  // 没见过的 kind 也得显示得出来：新告警源上线时前端可能还没跟上，
  // 而"有条告警但我不认识它"远好过"什么都不显示"
  const worded = S.alerts.kinds[g.kind];
  const lines = g.lines.map(line).filter((l): l is string => !!l);
  // count 数的是整组，lines 只带回前几条——差额是"还有 N 条"
  const rest = g.count - lines.length;
  return (
    // div 而不是 button：行里还有一个动作按钮，按钮套按钮是无效 HTML
    <div
      role="button"
      tabIndex={0}
      // **点击才算读过**，不是划过。鼠标经过一列告警不代表看过它们，
      // 而已读一旦落下就再也不会自己回来。点一下把这一组整个标掉
      onClick={() => {
        if (g.unread > 0) onRead(g);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" && g.unread > 0) onRead(g);
      }}
      className="u-row-shell flex w-full cursor-pointer gap-3 border-b border-line px-4 py-3 text-left last:border-b-0"
    >
      {/* 未读就是一个红点。整行描边或底色会让面板在告警多时变成一片红，
          而红点只占它该占的那一点地方，读过就没了 */}
      <span
        className={cn(
          "mt-[7px] h-1.5 w-1.5 rounded-full shrink-0",
          g.unread > 0 ? "bg-danger" : "bg-transparent",
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 flex-wrap">
          <span
            className={cn(
              "text-body",
              g.unread > 0 ? "font-medium text-ink" : "text-ink-2",
            )}
          >
            {worded?.title ?? S.alerts.unknownKind(g.kind)}
          </span>
          {g.count > 1 && <Chip tone="neutral">{g.count}</Chip>}
          <Chip tone={g.kb_name ? "neutral" : "violet"}>
            {g.kb_name ?? S.alerts.system}
          </Chip>
        </div>
        {worded && (
          <p className="mt-1 text-small text-ink-3">{worded.hint}</p>
        )}
        {lines.length > 0 && (
          <ul className="mt-1 space-y-1">
            {lines.map((l, i) => (
              <li key={i} className="text-fine text-ink-2 break-words">
                {l}
              </li>
            ))}
            {rest > 0 && (
              <li className="text-fine text-ink-3">
                {S.alerts.andMore(rest)}
              </li>
            )}
          </ul>
        )}
        {/* 时间取组里最新的那一次 */}
        <p className="u-num mt-2 text-fine text-ink-3">
          {new Date(g.latest_at).toLocaleString()}
        </p>
        {/* 修好之后接着跑：把这次故障窗口里失败的任务放回队列（#216）。
            余额耗尽是唯一一种「人做完一件具体的事就想让活继续」的失败，
            动作长在告警上，闭环就在这里，不必另建一个队列页 */}
        {REQUEUE_KINDS.has(g.kind) && (
          <Button variant="secondary" size="sm" className="mt-2"
            type="button"
            disabled={requeuing}
            onClick={(e) => {
              e.stopPropagation();
              onRequeue(g);
            }}
          >
            {S.alerts.runAgain}
          </Button>
        )}
      </div>
    </div>
  );
}

function Panel({ panelRef }: { panelRef: Ref<HTMLDivElement> }) {
  const [q, setQ] = useState("");
  const [page, setPage] = useState(0);
  const qc = useQueryClient();

  // 搜索后回第一页：停在第 4 页看一个只有 2 页的结果，
  // 面板会显示空白，而人会读成"没有告警"
  useEffect(() => {
    setPage(0);
  }, [q]);

  const list = useQuery({
    queryKey: ["alerts", "list", q, page],
    queryFn: () => api.alerts({ q, limit: PAGE, offset: page * PAGE }),
    // 翻页时留着上一页，免得面板高度塌一下再弹回来
    placeholderData: (prev) => prev,
  });

  const read = useMutation({
    mutationFn: (g: AlertGroup) =>
      api.alertReadGroup({
        kb_id: g.kb_id,
        kind: g.kind,
        from: g.earliest_at,
        to: g.latest_at,
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["alerts"] }),
  });
  const readAll = useMutation({
    mutationFn: () => api.alertsReadAll(),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["alerts"] }),
  });
  // 时间窗从这组最早那次故障起——之前失败的不是这次的事
  const requeue = useMutation({
    mutationFn: (g: AlertGroup) =>
      api.requeueJobs(g.kb_id, { failed_since: g.earliest_at }),
    onSuccess: (r) => {
      toast.success(S.alerts.requeued(r.requeued));
      qc.invalidateQueries({ queryKey: ["jobs"] });
    },
    onError: (e) => toast.error(String(e)),
  });

  const groups = list.data?.items ?? [];
  const total = list.data?.total ?? 0;

  return (
    // top-0 而不是 top-9：面板要从铃铛**原位**长出来，右上角对齐
    <div
      ref={panelRef}
      className="u-menu-glass absolute right-0 top-0 w-[420px] rounded-xl shadow-2xl z-50 overflow-hidden"
    >
      <div className="flex items-center gap-2 pl-4 pr-8 py-3 border-b border-line">
        <span className="text-body font-medium text-ink">
          {S.alerts.title}
        </span>
      </div>

      {/* 跟文库的过滤框同一套：input-dark + 左侧图标 + 有值时右侧清除、Esc 清空 */}
      <div className="px-4 py-3 border-b border-line">
        <div className="relative">
          <Search
            size={13}
            className="absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-3 pointer-events-none"
          />
          <Input size="sm" className="w-full pl-8 pr-8"
            placeholder={S.alerts.searchPlaceholder}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setQ("")}
          />
          {q && (
            <IconButton size="sm" label={S.ui.close} className="absolute right-2 top-1/2 -translate-y-1/2"
              onClick={() => setQ("")}
            >
              <X size={12} />
            </IconButton>
          )}
        </div>
      </div>

      <div className="max-h-[420px] overflow-y-auto">
        {groups.length === 0 ? (
          <div className="px-4 py-8 text-center">
            <p className="text-body text-ink-2">
              {q ? S.alerts.noMatch : S.alerts.empty}
            </p>
            {!q && (
              <p className="mt-1 text-small text-ink-3">
                {S.alerts.emptyHint}
              </p>
            )}
          </div>
        ) : (
          groups.map((g) => (
            <AlertRow
              key={`${g.kb_id ?? "system"}|${g.kind}|${g.latest_at}`}
              g={g}
              onRead={(x) => read.mutate(x)}
              onRequeue={(x) => requeue.mutate(x)}
              requeuing={requeue.isPending}
            />
          ))
        )}
      </div>

      {/* 底栏：整张列表级的动作跟翻页放一起，离光标最远 */}
      {groups.length > 0 && (
        <div className="flex items-center gap-3 px-4 py-2 border-t border-line">
          {groups.some((g) => g.unread > 0) && (
            <LinkButton onClick={() => readAll.mutate()}>
              {S.alerts.markAllRead}
            </LinkButton>
          )}
          <Pager
            className="ml-auto"
            total={total}
            pageSize={PAGE}
            page={page}
            onPage={setPage}
          />
        </div>
      )}
    </div>
  );
}

export function AlertBell() {
  // 跟用户菜单同一份原地变形：两个面板紧挨着，动画差一点点来回点两下就看得出来
  const { open, setOpen, close, rootRef, anchorRef, panelRef } =
    usePopoverFlip<HTMLButtonElement, HTMLDivElement>();
  const unread = useQuery({
    queryKey: ["alerts", "unread"],
    queryFn: () => api.alertsUnread(),
    // 推送是主路，这个只是断流时的兜底
    refetchInterval: 120_000,
  });
  const n = unread.data?.unread ?? 0;

  return (
    <div ref={rootRef} className="relative">
      <IconButton
        size="sm"
        ref={anchorRef}
        label={S.alerts.badgeLabel}
        aria-expanded={open}
        className={cn("relative", open && "bg-surface-2 text-ink")}
        onClick={() => (open ? close() : setOpen(true))}
      >
        <Bell size={15} />
        {/* 角标也是个点，不是数字。"有事没看"是二元的，具体几条打开就知道；
            数字还会随重试一路往上跳，跳到三位数就把铃铛撑变形了 */}
        {n > 0 && (
          <span className="absolute top-1 right-1 h-1.5 w-1.5 rounded-full bg-danger" />
        )}
      </IconButton>
      {open && (
        <>
          <Panel panelRef={panelRef} />
          {/* 关闭按钮是面板的**兄弟**，不是它的孩子：放里面的话 `right-0 top-0`
              相对的是面板的内边距盒，而 u-menu-glass 有一条 0.667px 的发丝边框
              （DPR 1.5 上的一个物理像素），永远差那么一点。放在这里，定位祖先
              就是裹着铃铛的这个 div，跟铃铛同一个盒子——重合是构造出来的。

              光标点开面板之后正停在这个位置，所以这儿必须是"再点一下关掉"。
              放"全部标为已读"等于把误触做成默认动作，而它一下清掉的是
              所有库的所有告警 */}
          <IconButton
            size="sm"
            label={S.alerts.close}
            className="absolute right-0 top-0 z-[60]"
            onClick={close}
          >
            <X size={15} />
          </IconButton>
        </>
      )}
    </div>
  );
}
