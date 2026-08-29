// 告警中心（0005）。**Review 管知识的对错，告警管系统的死活。**
//
// 一条告警是聚合体：12 份扫描件是一条，不是 12 条。所以标题带数量，
// 详情列在下面。
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, CheckCircle2, Info, XCircle } from "lucide-react";

import { api, type Alert } from "../api";
import { S } from "../i18n";
import { Chip, PageTitle, cn } from "../ui";

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

function AlertCard({ a, onRead }: { a: Alert; onRead: (id: string) => void }) {
  const { title, hint } = heading(a);
  const Icon = a.resolved_at ? CheckCircle2 : ICON[a.severity];
  const lines = detailLines(a);
  return (
    <div
      // 未读靠左侧一道竖线，不靠底色——底色会让整页在告警多时变成一片红
      className={cn(
        "rounded-xl glass p-4 flex gap-3",
        !a.read && !a.resolved_at && "border-l-2 border-l-rose-500/70",
        a.resolved_at && "opacity-55",
      )}
      onMouseEnter={() => {
        if (!a.read) onRead(a.id);
      }}
    >
      <Icon
        size={16}
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
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium text-white">{title}</span>
          <Chip tone={a.kb_name ? "neutral" : "violet"}>
            {a.kb_name ?? S.alerts.system}
          </Chip>
          {a.resolved_at && <Chip tone="success">{S.alerts.resolved}</Chip>}
        </div>
        {hint && !a.resolved_at && (
          <p className="mt-1 text-[12.5px] text-neutral-500">{hint}</p>
        )}
        {lines.length > 0 && (
          <ul className="mt-2 space-y-0.5">
            {lines.map((l, i) => (
              <li key={i} className="text-[12px] text-neutral-400 break-words">
                {l}
              </li>
            ))}
          </ul>
        )}
        <p className="u-num mt-2 text-[11px] text-neutral-600">
          {new Date(a.last_seen).toLocaleString()}
        </p>
      </div>
    </div>
  );
}

export function Alerts() {
  const [showResolved, setShowResolved] = useState(false);
  const qc = useQueryClient();
  const list = useQuery({
    queryKey: ["alerts", "list", showResolved],
    queryFn: () => api.alerts(showResolved),
  });

  const read = useMutation({
    mutationFn: (id: string) => api.alertRead(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["alerts"] }),
  });
  const readAll = useMutation({
    mutationFn: () => api.alertsReadAll(),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["alerts"] }),
  });

  const rows = list.data ?? [];
  const unread = rows.filter((a) => !a.read && !a.resolved_at).length;

  return (
    <div className="p-6 max-w-3xl mx-auto w-full overflow-y-auto">
      <div className="flex items-center gap-3 mb-5">
        <PageTitle>{S.alerts.title}</PageTitle>
        {unread > 0 && <Chip tone="danger">{unread}</Chip>}
        <div className="ml-auto flex items-center gap-2">
          <button
            className="text-[12.5px] text-neutral-500 hover:text-neutral-200 transition-colors"
            onClick={() => setShowResolved((v) => !v)}
          >
            {showResolved ? S.alerts.hideResolved : S.alerts.showResolved}
          </button>
          {unread > 0 && (
            <button
              className="text-[12.5px] text-neutral-500 hover:text-neutral-200 transition-colors"
              onClick={() => readAll.mutate()}
            >
              {S.alerts.markAllRead}
            </button>
          )}
        </div>
      </div>

      {rows.length === 0 ? (
        <div className="rounded-xl glass p-8 text-center">
          <p className="text-sm text-neutral-300">{S.alerts.empty}</p>
          <p className="mt-1 text-[12.5px] text-neutral-500">
            {S.alerts.emptyHint}
          </p>
        </div>
      ) : (
        <div className="space-y-2.5">
          {rows.map((a) => (
            <AlertCard key={a.id} a={a} onRead={(id) => read.mutate(id)} />
          ))}
        </div>
      )}
    </div>
  );
}
