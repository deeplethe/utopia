// 正在生成的那一次回答，活在组件之外。
//
// **切走一次就看不见了。** 流式中的 `turns` 从前是 Chat 的组件状态，而离开
// 对话页会卸载这个组件：状态没了，那个 fetch 还在跑，回调写进的是一个已经
// 死掉的组件。切回来时组件重新挂载、从库里读——库里要等生成结束才有那一行，
// 于是只看得见自己问的那句话。等一会儿再回来就正常，因为那时已经落库了。
//
// 服务端那半边（生成不随连接消失）是另一条修复；这半边解决的是**回来的时候
// 看不看得见**。两条缺一不可：服务端保住了答案，这里保住了那条流。
//
// 同时只会有一次进行中的回答——所以这是个单例，不是按会话开的表。
import type { ChatStep, Source } from "./api";

export interface Turn {
  role: "user" | "assistant";
  content: string;
  steps?: ChatStep[];
  sources?: Source[];
  error?: string;
}

interface Live {
  conversationId: string | null;
  turns: Turn[];
  /** 停止按钮用；切页面**不调它**，那正是这次修复的意思 */
  abort: () => void;
  /** 还在写，还是已经写完。**写完不清空**——见 `finish` */
  streaming: boolean;
}

let live: Live | null = null;
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((l) => l());
}

export const liveAnswer = {
  /** `useSyncExternalStore` 要求同一个快照对象在没变时保持同一引用 */
  get: () => live,
  subscribe: (l: () => void) => {
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  },
  start: (conversationId: string | null, turns: Turn[], abort: () => void) => {
    live = { conversationId, turns, abort, streaming: true };
    emit();
  },
  /** 会话是流式中途新建的：`onConversation` 回来才知道 id */
  identify: (conversationId: string) => {
    if (!live) return;
    live = { ...live, conversationId };
    emit();
  },
  /** 改最后一轮（助手那一轮）。生成期间只有它在变 */
  patchLast: (f: (t: Turn) => Turn) => {
    if (!live) return;
    const turns = [...live.turns];
    turns[turns.length - 1] = f(turns[turns.length - 1]);
    live = { ...live, turns };
    emit();
  },
  /** 结束（正常、出错、或人按了停止）。
   *
   * **不清空。** 清空过一版，那一版有个很难看的 bug：切走时组件卸载，
   * 而「把最终结果交回组件」是调在已经死掉的那个组件上——空操作。于是
   * store 空了、新组件早前已经认领过这一场因而不会再去读库，切回来
   * 整场对话一片空白，连自己问的那句都没有。
   *
   * 那一刻这里是唯一还握着这份内容的地方，所以留着：只把 `streaming`
   * 落下来。下一次 `start` 会替掉它，切到别的会话时 id 对不上自然不显示。
   */
  finish: () => {
    if (!live) return;
    live = { ...live, streaming: false };
    emit();
  },
};
