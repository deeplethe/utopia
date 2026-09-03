// 正在生成中的那些回答，活在组件之外。
//
// **切走一次就看不见了。** 流式中的 `turns` 从前是 Chat 的组件状态，而离开
// 对话页会卸载这个组件：状态没了，那个 fetch 还在跑，回调写进的是一个已经
// 死掉的组件。切回来时组件重新挂载、从库里读——库里要等生成结束才有那一行，
// 于是只看得见自己问的那句话。等一会儿再回来就正常，因为那时已经落库了。
//
// 服务端那半边（生成不随连接消失）是另一条修复；这半边解决的是**回来的时候
// 看不看得见**。两条缺一不可：服务端保住了答案，这里保住了那条流。
//
// **按会话键控，不是单例。** 这张表从前是一个槽位，依据是「同时只会有一次
// 进行中的回答」。这个前提不成立，而且是被 Chat 自己否定的——换库不 abort
// （「换库不该杀掉另一个库里正在写的回答」）、开新对话不 abort（「开一场新的
// 不等于放弃上一场」）、发送守卫按会话收窄（明确拒绝「发不出消息」的全局封锁）。
// 三个「不 abort」凑在一起，两场并发是常规可达的状态，而单槽装不下它：第二场
// start 覆盖槽位，第一场的回调还在往「当前槽位的最后一条」里写，两场回答逐字
// 交织；先结束的那场把另一场的停止按钮提前收掉，自己从此无人可停。
//
// 于是改成一张表：谁开场谁拿句柄，读谁写谁都有名有姓。`send` 的守卫不用改——
// 它本来问的就是「这一场在不在流」，现在这个问题终于只关于这一场。
import type { ChatStep, Source } from "./api";

export interface Turn {
  role: "user" | "assistant";
  content: string;
  steps?: ChatStep[];
  sources?: Source[];
  error?: string;
}

/** 快照条目：纯数据，给渲染看。abort 不进快照——渲染不该顺手摸到它 */
export interface Live {
  kbId: string;
  /** 新会话在服务端回 id 之前是 null；kbId 用来区分两个都还没拿到 id 的新会话 */
  conversationId: string | null;
  turns: Turn[];
  streaming: boolean;
}

interface Slot {
  live: Live;
  abort: () => void;
}

const lives = new Map<string, Slot>();
const listeners = new Set<() => void>();

// 快照整体替换：useSyncExternalStore 靠引用相等跳过无关渲染——**别场的任何
// 变更都不该改变这一场的画面**，这条旧注释在键控之后才字面成立。
let snapshot: readonly Live[] = [];

function emit() {
  snapshot = [...lives.values()].map((s) => s.live);
  listeners.forEach((l) => l());
}

// 还没拿到 id 的新会话用内部 token 占位；identify 到真 id 时重映射
let pendingSeq = 0;

export interface LiveHandle {
  /** 新会话从服务端拿到 id：把这个条目从占位 token 重映射到真 id */
  identify: (conversationId: string) => void;
  /** 改这场回答的最后一条（助手那一轮）。生成期间只有它在变 */
  patchLast: (f: (t: Turn) => Turn) => void;
  /** 结束（正常、出错、或人按了停止）。
   *
   * **不清空。** 清空过一版，那一版有个很难看的 bug：切走时组件卸载，
   * 而「把最终结果交回组件」是调在已经死掉的那个组件上——空操作。于是
   * store 空了、新组件早前已经认领过这一场因而不会再去读库，切回来
   * 整场对话一片空白，连自己问的那句都没有。
   *
   * 那一刻这里是唯一还握着这份内容的地方，所以留着：只把 `streaming`
   * 落下来。下一次 `begin` 会清掉已结束的条目（见 begin），切到别的会话时
   * 认领不上自然去读库。 */
  finish: () => void;
  /** streamChat 的 abort 要等它返回才有：begin 先给占位，拿到真 abort 再换上 */
  setAbort: (abort: () => void) => void;
}

export const liveAnswer = {
  /** `useSyncExternalStore` 要求同一个快照对象在没变时保持同一引用 */
  get: (): readonly Live[] => snapshot,
  subscribe: (l: () => void) => {
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  },
  /** 认领「正在看的这一场」。按会话找；kbId 只在两个都还没拿到 id 的新会话
      之间起区分作用。找不到就是这一场不在场——展示回落到库里的历史 */
  entry: (kbId: string | null, conversationId: string | null): Live | null =>
    snapshot.find((e) => e.kbId === kbId && e.conversationId === conversationId) ?? null,
  /** 开一场。同会话追问会替换同 key 的旧条目；同时清掉所有已结束的条目——
   *
   * 清除只在有人发新消息时发生，而被清的会话若再被打开，认领不上、自然去
   * 读库，内容一致（服务端在 done 时已落库）。不清的话这张表无界增长；
   * 进行中的条目永不清——那正是本模块存在的意义。 */
  begin: (
    kbId: string,
    conversationId: string | null,
    turns: Turn[],
    abort: () => void,
  ): LiveHandle => {
    for (const [k, s] of lives) if (!s.live.streaming) lives.delete(k);
    let key = conversationId ?? `__pending__${++pendingSeq}`;
    const slot: Slot = { live: { kbId, conversationId, turns, streaming: true }, abort };
    lives.set(key, slot);
    emit();
    return {
      identify: (id: string) => {
        const current = lives.get(key);
        if (!current) return;
        lives.delete(key);
        key = id;
        current.live = { ...current.live, conversationId: id };
        lives.set(key, current);
        emit();
      },
      patchLast: (f) => {
        const current = lives.get(key);
        if (!current || current.live.turns.length === 0) return;
        const turns = [...current.live.turns];
        turns[turns.length - 1] = f(turns[turns.length - 1]);
        current.live = { ...current.live, turns };
        emit();
      },
      finish: () => {
        const current = lives.get(key);
        if (!current || !current.live.streaming) return;
        current.live = { ...current.live, streaming: false };
        emit();
      },
      setAbort: (a) => {
        const current = lives.get(key);
        if (current) current.abort = a;
      },
    };
  },
  /** 停止按钮专用：abort + finish 正在看的这一场。别的场照常写它们自己的条目 */
  stop: (kbId: string, conversationId: string | null) => {
    for (const s of lives.values()) {
      if (s.live.kbId === kbId && s.live.conversationId === conversationId) {
        s.abort();
        s.live = { ...s.live, streaming: false };
        emit();
        return;
      }
    }
  },
};
