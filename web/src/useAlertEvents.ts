// 告警事件订阅。**全局，不按库**——顶栏角标是跨库的，而系统级告警根本没有库。
//
// 服务端推的那条不带任何数据也不判权限（见 alerts_routes::stream）：收到就重取，
// 谁能看见什么由列表查询说了算。所以这里也不需要知道当前是哪个库。
import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";

export function useAlertEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    const es = new EventSource("/api/v1/alerts/events");
    es.addEventListener("alert", () => {
      queryClient.invalidateQueries({ queryKey: ["alerts"] });
    });
    return () => es.close();
  }, [queryClient]);
}
