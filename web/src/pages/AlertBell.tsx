// 顶栏告警角标（0005）。
//
// 角标数是**我的未读**：可见的、未解决的、我没读过的。
// 「已读」逐人，「已解决」全局——一个人读过不代表事情解决了。
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Bell } from "lucide-react";

import { api } from "../api";
import { S } from "../i18n";

export function AlertBell() {
  const unread = useQuery({
    queryKey: ["alerts", "unread"],
    queryFn: () => api.alertsUnread(),
    // 推送是主路，这个只是断流时的兜底
    refetchInterval: 120_000,
  });
  const n = unread.data?.unread ?? 0;
  return (
    <Link
      to="/alerts"
      title={S.alerts.badgeLabel}
      aria-label={S.alerts.badgeLabel}
      className="relative flex items-center rounded-lg px-2 py-1 text-neutral-500 hover:text-neutral-200 hover:bg-white/[0.05] transition-colors"
    >
      <Bell size={15} />
      {n > 0 && (
        // 99+ 而不是真数字：三位数会把角标撑变形，而"多少条"到这个量级已经不重要
        <span className="u-num absolute -top-0.5 -right-0.5 min-w-[15px] h-[15px] px-1 rounded-full bg-rose-500/90 text-[10px] leading-[15px] text-white text-center">
          {n > 99 ? "99+" : n}
        </span>
      )}
    </Link>
  );
}
