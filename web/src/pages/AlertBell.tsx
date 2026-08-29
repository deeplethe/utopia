// 顶栏告警（0005）：铃铛 + 未读角标 + 弹出面板。
//
// **弹窗不是页面**：告警是"顺手瞄一眼"的东西，不是一个要专门去逛的地方。
// 做成页面会逼人离开手头的事，而离开的代价就是没人去看。
//
// 角标数是**我的未读**：可见的、未解决的、我没读过的。
// 「已读」逐人，「已解决」全局——一个人读过不代表事情解决了。
import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Bell,
  CheckCircle2,
  Info,
  Search,
  XCircle,
} from "lucide-react";

import { api, type Alert } from "../api";
import { S } from "../i18n";
import { Chip, Pager, cn } from "../ui";

const PAGE = 8;

/** 详情最多列几条——一条告警可以聚合上百个对象，面板里塞不下也不该塞 */
const MAX_LINES = 4;

const ICON: Record<Alert["severity"], typeof Info> = {
  info: Info,
  warning: AlertTriangle,
  error: XCircle,
};

/** 每条详情一行。系统级的 detail 是 `{ error }`，库级的按 subject id 索引 */
function detailLines(a: Alert): string[] {
  const out: string[] = [];
  for (const [k, v] of Object.entries(a.detail)) {
    if (k === "error" && typeof v === "string") {
      out.push(v);
      continue;
    }
    const row = v as { name?: string; error?: string } | null;
    if (!row || typeof row !== "object") continue;
    out.push([row.name, row.error].filter(Boolean).join(" — "));
  }
  return out;
}

function heading(a: Alert): { title: string; hint: string | null } {
  const n = a.subject_ids.length;
  const worded = S.alerts.kinds[a.kind];
  // 没见过的 kind 也得显示得出来：新告警源上线时前端可能还没跟上，
  // 而"有条告警但我不认识它"远好过"什么都不显示"
  if (!worded) return { title: S.alerts.unknownKind(a.kind, n), hint: null };
  // 已解决的不报数量：对象清空正是它解决的原因，n 恒为 0
  if (a.resolved_at) return { title: worded.resolved, hint: null };
  return { title: worded.title(n), hint: worded.hint };
}

function AlertRow({ a, onRead }: { a: Alert; onRead: (id: string) => void }) {
  const { title, hint } = heading(a);
  const Icon = a.resolved_at ? CheckCircle2 : ICON[a.severity];
  const lines = detailLines(a);
  const shown = lines.slice(0, MAX_LINES);
  const rest = lines.length - shown.length;
  return (
    <div
      // 未读靠左侧一道竖线，不靠底色——底色会让面板在告警多时变成一片红
      className={cn(
        "flex gap-2.5 px-3.5 py-3 border-b border-white/[0.06] last:border-b-0",
        !a.read && !a.resolved_at && "border-l-2 border-l-rose-500/70",
        a.resolved_at && "opacity-55",
      )}
      onMouseEnter={() => {
        if (!a.read) onRead(a.id);
      }}
    >
      <Icon
        size={14}
        className={cn(
          "mt-0.5 shrink-0",
          a.resolved_at
            ? "text-emerald-400/70"
            : a.severity === "error"
              ? "text-rose-400"
              : "text-amber-400",
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5 flex-wrap">
          <span className="text-[13px] font-medium text-white">{title}</span>
          <Chip tone={a.kb_name ? "neutral" : "violet"}>
            {a.kb_name ?? S.alerts.system}
          </Chip>
          {a.resolved_at && <Chip tone="success">{S.alerts.resolved}</Chip>}
        </div>
        {hint && <p className="mt-0.5 text-[11.5px] text-neutral-500">{hint}</p>}
        {shown.length > 0 && (
          <ul className="mt-1.5 space-y-0.5">
            {shown.map((l, i) => (
              <li key={i} className="text-[11px] text-neutral-400 break-words">
                {l}
              </li>
            ))}
            {rest > 0 && (
              <li className="text-[11px] text-neutral-600">
                {S.alerts.andMore(rest)}
              </li>
            )}
          </ul>
        )}
        <p className="u-num mt-1.5 text-[10.5px] text-neutral-600">
          {new Date(a.last_seen).toLocaleString()}
        </p>
      </div>
    </div>
  );
}

function Panel() {
  const [q, setQ] = useState("");
  const [page, setPage] = useState(0);
  const [showResolved, setShowResolved] = useState(false);
  const qc = useQueryClient();

  // 搜索或切筛选后回第一页：停在第 4 页看一个只有 2 页的结果，
  // 面板会显示空白，而人会读成"没有告警"
  useEffect(() => {
    setPage(0);
  }, [q, showResolved]);

  const list = useQuery({
    queryKey: ["alerts", "list", q, showResolved, page],
    queryFn: () =>
      api.alerts({
        q,
        includeResolved: showResolved,
        limit: PAGE,
        offset: page * PAGE,
      }),
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
    <div className="u-menu-glass absolute right-0 top-9 w-[420px] rounded-xl shadow-2xl z-50 overflow-hidden">
      <div className="flex items-center gap-2 px-3.5 py-2.5 border-b border-white/10">
        <span className="text-[13px] font-medium text-neutral-100">
          {S.alerts.title}
        </span>
        <div className="ml-auto flex items-center gap-2.5">
          <button
            className="text-[11.5px] text-neutral-500 hover:text-neutral-200 transition-colors"
            onClick={() => setShowResolved((v) => !v)}
          >
            {showResolved ? S.alerts.hideResolved : S.alerts.showResolved}
          </button>
          <button
            className="text-[11.5px] text-neutral-500 hover:text-neutral-200 transition-colors"
            onClick={() => readAll.mutate()}
          >
            {S.alerts.markAllRead}
          </button>
        </div>
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
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const unread = useQuery({
    queryKey: ["alerts", "unread"],
    queryFn: () => api.alertsUnread(),
    // 推送是主路，这个只是断流时的兜底
    refetchInterval: 120_000,
  });
  const n = unread.data?.unread ?? 0;

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node))
        setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
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
        {n > 0 && (
          // 99+ 而不是真数字：三位数会把角标撑变形，而到这个量级"多少条"已经不重要
          <span className="u-num absolute -top-0.5 -right-0.5 min-w-[15px] h-[15px] px-1 rounded-full bg-rose-500/90 text-[10px] leading-[15px] text-white text-center">
            {n > 99 ? "99+" : n}
          </span>
        )}
      </button>
      {open && <Panel />}
    </div>
  );
}
