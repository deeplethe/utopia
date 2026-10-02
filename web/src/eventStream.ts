// 一条通知用的事件流：断线恢复时补刷，页面藏起来一阵后把连接让出来（#1028）。
//
// 浏览器给同一个源的 HTTP/1.1 连接只有六条，事件流一条占一条、占着不放。从前每个标签页
// 开两条通知流（当前库的事件、告警），正在生成的回答再占一条：两个标签页就把六条占满，
// Stop 这样的普通请求排在后面发不出去；三个空闲的标签页也一样。所以一页只开一条通知流
// （库事件那条顺带送告警，见 `useKbEvents`），藏起来超过 `HIDDEN_RELEASE_MS` 的页把它
// 让出来，回到前台再连上。
//
// 事件没有回放：断线期间、让出期间的变化都收不到，所以重新连上时调一次 `onRecover` 补刷。
// 首次正常连接不补刷，页面自己的读取已经是新的。

/** 页面藏起来多久之后让出连接。短暂切走再回来不断流，也就不用补刷 */
export const HIDDEN_RELEASE_MS = 30_000;

export function subscribeEvents(
  url: string,
  handlers: Record<string, () => void>,
  onRecover: () => void,
): () => void {
  const page = typeof document === "undefined" ? null : document;
  let source: EventSource | null = null;
  let missed = false;
  let disposed = false;
  let release: ReturnType<typeof setTimeout> | null = null;

  // 卸载之后迟到的回调一律不作数：旧连接的事件不能再去失效新页面的查询
  const guarded = Object.entries(handlers).map(([type, handler]): [string, () => void] => [
    type,
    () => {
      if (!disposed) handler();
    },
  ]);
  const listeners: Record<string, () => void> = {
    ...Object.fromEntries(guarded),
    error: () => {
      if (!disposed) missed = true;
    },
    open: () => {
      if (disposed || !missed) return;
      missed = false;
      onRecover();
    },
  };
  const open = () => {
    if (disposed || source) return;
    source = new EventSource(url);
    for (const [type, listener] of Object.entries(listeners)) source.addEventListener(type, listener);
  };
  const close = () => {
    if (!source) return;
    for (const [type, listener] of Object.entries(listeners)) source.removeEventListener(type, listener);
    source.close();
    source = null;
  };
  const cancelRelease = () => {
    if (release === null) return;
    clearTimeout(release);
    release = null;
  };
  const onVisibility = () => {
    cancelRelease();
    if (page?.visibilityState !== "hidden") {
      open();
      return;
    }
    release = setTimeout(() => {
      release = null;
      if (!source) return;
      close();
      missed = true;
    }, HIDDEN_RELEASE_MS);
  };

  page?.addEventListener("visibilitychange", onVisibility);
  if (page?.visibilityState === "hidden") missed = true;
  else open();

  return () => {
    disposed = true;
    page?.removeEventListener("visibilitychange", onVisibility);
    cancelRelease();
    close();
  };
}
