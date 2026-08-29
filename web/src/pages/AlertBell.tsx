// 顶栏告警（0005）：铃铛 + 未读角标 + 弹出面板。
//
// **弹窗不是页面**：告警是"顺手瞄一眼"的东西，不是一个要专门去逛的地方。
// 做成页面会逼人离开手头的事，而离开的代价就是没人去看。
//
// 一条告警 = 一次故障，写完不再变，没有"已解决"。
// 「已读」逐人——一个人读过不代表别人也该从未读里消失。
import { type Ref, useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Bell, Search } from "lucide-react";

import { api, type Alert } from "../api";
import { S } from "../i18n";
import { Chip, Pager, cn } from "../ui";
import { usePopoverFlip } from "../ui/popoverFlip";

const PAGE = 8;

/** 详情里给人看的那一行：对象名 — 报错原文 */
function detailLine(a: Alert): string | null {
  const d = a.detail as { name?: string; error?: string; job?: string };
  const parts = [d.name ?? d.job, d.error].filter(Boolean);
  return parts.length ? parts.join(" — ") : null;
}

function AlertRow({ a, onRead }: { a: Alert; onRead: (id: string) => void }) {
  // 没见过的 kind 也得显示得出来：新告警源上线时前端可能还没跟上，
  // 而"有条告警但我不认识它"远好过"什么都不显示"
  const worded = S.alerts.kinds[a.kind];
  const line = detailLine(a);
  return (
    <button
      type="button"
      // **点击才算读过**，不是划过。鼠标经过一列告警不代表看过它们，
      // 而已读一旦落下就再也不会自己回来
      onClick={() => {
        if (!a.read) onRead(a.id);
      }}
      className="w-full text-left flex gap-2.5 px-3.5 py-3 border-b border-white/[0.06] last:border-b-0 hover:bg-white/[0.03] transition-colors"
    >
      {/* 未读就是一个红点。整行描边或底色会让面板在告警多时变成一片红，
          而红点只占它该占的那一点地方，读过就没了 */}
      <span
        className={cn(
          "mt-[7px] h-1.5 w-1.5 rounded-full shrink-0",
          a.read ? "bg-transparent" : "bg-rose-500",
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5 flex-wrap">
          <span
            className={cn(
              "text-[13px]",
              a.read ? "text-neutral-400" : "font-medium text-white",
            )}
          >
            {worded?.title ?? S.alerts.unknownKind(a.kind)}
          </span>
          <Chip tone={a.kb_name ? "neutral" : "violet"}>
            {a.kb_name ?? S.alerts.system}
          </Chip>
        </div>
        {worded && (
          <p className="mt-0.5 text-[11.5px] text-neutral-500">{worded.hint}</p>
        )}
        {line && (
          <p className="mt-1 text-[11px] text-neutral-400 break-words">{line}</p>
        )}
        <p className="u-num mt-1.5 text-[10.5px] text-neutral-600">
          {new Date(a.created_at).toLocaleString()}
        </p>
      </div>
    </button>
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
    mutationFn: (id: string) => api.alertRead(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["alerts"] }),
  });
  const readAll = useMutation({
    mutationFn: () => api.alertsReadAll(),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["alerts"] }),
  });

  const rows = list.data?.items ?? [];
  const total = list.data?.total ?? 0;

  return (
    // top-0 而不是 top-9：面板要从铃铛**原位**长出来，右上角对齐
    <div
      ref={panelRef}
      className="u-menu-glass absolute right-0 top-0 w-[420px] rounded-xl shadow-2xl z-50 overflow-hidden"
    >
      <div className="flex items-center gap-2 px-3.5 py-2.5 border-b border-white/10">
        <span className="text-[13px] font-medium text-neutral-100">
          {S.alerts.title}
        </span>
        <button
          className="ml-auto text-[11.5px] text-neutral-500 hover:text-neutral-200 transition-colors"
          onClick={() => readAll.mutate()}
        >
          {S.alerts.markAllRead}
        </button>
      </div>

      <div className="flex items-center gap-2 px-3.5 py-2 border-b border-white/[0.06]">
        <Search size={13} className="text-neutral-600 shrink-0" />
        <input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder={S.alerts.searchPlaceholder}
          className="w-full bg-transparent text-[12.5px] text-neutral-200 placeholder:text-neutral-600 outline-none"
        />
      </div>

      <div className="max-h-[420px] overflow-y-auto">
        {rows.length === 0 ? (
          <div className="px-3.5 py-8 text-center">
            <p className="text-[13px] text-neutral-300">
              {q ? S.alerts.noMatch : S.alerts.empty}
            </p>
            {!q && (
              <p className="mt-1 text-[11.5px] text-neutral-500">
                {S.alerts.emptyHint}
              </p>
            )}
          </div>
        ) : (
          rows.map((a) => (
            <AlertRow key={a.id} a={a} onRead={(id) => read.mutate(id)} />
          ))
        )}
      </div>

      {total > PAGE && (
        <div className="px-3.5 pb-2.5">
          <Pager total={total} pageSize={PAGE} page={page} onPage={setPage} />
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
      <button
        ref={anchorRef}
        onClick={() => (open ? close() : setOpen(true))}
        title={S.alerts.badgeLabel}
        aria-label={S.alerts.badgeLabel}
        aria-expanded={open}
        className={cn(
          "relative flex items-center rounded-lg px-2 py-1 transition-colors",
          open
            ? "text-neutral-200 bg-white/[0.06]"
            : "text-neutral-500 hover:text-neutral-200 hover:bg-white/[0.05]",
        )}
      >
        <Bell size={15} />
        {/* 角标也是个点，不是数字。"有事没看"是二元的，具体几条打开就知道；
            数字还会随重试一路往上跳，跳到三位数就把铃铛撑变形了 */}
        {n > 0 && (
          <span className="absolute top-0.5 right-1 h-1.5 w-1.5 rounded-full bg-rose-500" />
        )}
      </button>
      {open && <Panel panelRef={panelRef} />}
    </div>
  );
}
