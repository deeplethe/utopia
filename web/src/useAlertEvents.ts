// 告警事件订阅。**全局，不按库**——顶栏角标是跨库的，而系统级告警根本没有库。
//
// 服务端推的那条不带任何数据也不判权限（见 alerts_routes::stream）：收到就重取，
// 谁能看见什么由列表查询说了算。所以这里也不需要知道当前是哪个库。
//
// 打开着一个库的页不用这条：库事件流顺带送告警（`useKbEvents`），一页只占一条连接
// （#1028）。`active` 为假时什么都不订。
import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { subscribeEvents } from "./eventStream";

export function useAlertEvents(active = true) {
  const queryClient = useQueryClient();
  useEffect(() => {
    if (!active) return;
    // 断线期间的告警不会回放：恢复时同时补刷角标与列表。
    const refresh = () => queryClient.invalidateQueries({ queryKey: ["alerts"] });
    return subscribeEvents("/api/v1/alerts/events", { alert: refresh }, refresh);
  }, [active, queryClient]);
}
