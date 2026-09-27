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
import { conversationsApi, type ChatStep, type Source } from "./api";
import { citedNumbers } from "./citations";

export interface Turn {
  role: "user" | "assistant";
  /** 这一条存下的 id。问题要有它才能重答（#936）：库里的历史带着，刚发出的那一问
   *  由服务端的 `conversation` 帧告诉 */
  id?: string;
  content: string;
  steps?: ChatStep[];
  sources?: Source[];
  error?: string;
  stopped?: boolean;
}

/** 正文里真正引到的那几条来源。
 *
 *  `sources` 是这一轮**检索到**的全部，不是回答**用到**的：打个招呼也可能顺手搜了
 *  一次，六条摘录挂在「你好」下面，读起来像是这句问候有六个出处。所以只列正文里
 *  画成了角标的那几条，编号照原样不重排，与正文里的标记对得上。
 *  `[1][2]`、`[1, 2]`、`[1，2]` 都认；代码与链接里的方括号不算，正文里也不画 */
export function citedSources(turn: Turn): Source[] {
  if (!turn.sources?.length) return [];
  const cited = citedNumbers(turn.content);
  return turn.sources.filter((s) => cited.has(s.n));
}

/** 这条回答要不要挂「未引用任何来源」（#547）。
 *
 *  判据只看数据：说完了、正文一条来源都没引，就挂——招呼、拒答挂着无害，
 *  而「以下是我找到的内容」配零引用的那条，靠它露馅。检索到了却一条没引，
 *  同样算没有来源。还在流的不挂：来源是增量到的，挂上又撤下比晚一点出现更糟。
 *  只有报错、一个字没说的那条也不挂，它不是回答，红字已经交代了 */
export function answeredWithoutSources(turn: Turn, live: boolean): boolean {
  if (turn.role !== "assistant" || live) return false;
  if ((turn.error || turn.stopped) && !turn.content) return false;
  return citedSources(turn).length === 0;
}

/** 快照条目：纯数据，给渲染看。abort 不进快照——渲染不该顺手摸到它 */
export interface Live {
  kbId: string;
  /** 新会话在服务端回 id 之前是 null；kbId 用来区分两个都还没拿到 id 的新会话 */
  conversationId: string | null;
  generationId: string | null;
  turns: Turn[];
  streaming: boolean;
  stopping: boolean;
  stopError?: string;
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

/* 通知**按帧合并**。一次生成里词元是一个一个来的，每个都通知一次，React 就
   一个词元渲染一遍；答案长到几千字之后，渲染跟不上词元，画面看着是一顿一顿地
   往外蹦字。33ms 一次（30 次/秒）对读字来说绰绰有余，而渲染次数降了一个量级。

   用 setTimeout 不用 requestAnimationFrame：标签页切到后台时 rAF 会停，
   而这个 store 明确支持"切走再切回来"——停了就得等回到前台才结算。

   结构性的改动（开始、结束、认领到真 id）走 `flush`，立刻通知：它们不是
   连续来的，也不该等下一帧。 */
const NOTIFY_MS = 33;
let pending: ReturnType<typeof setTimeout> | null = null;

function notify() {
  snapshot = [...lives.values()].map((s) => s.live);
  listeners.forEach((l) => l());
}

function emit() {
  if (pending) return;
  pending = setTimeout(() => {
    pending = null;
    notify();
  }, NOTIFY_MS);
}

function flush() {
  if (pending) {
    clearTimeout(pending);
    pending = null;
  }
  notify();
}

async function requestStop(slot: Slot) {
  const { kbId, conversationId, generationId } = slot.live;
  if (!conversationId || !generationId) return; // 首帧前的停止意图由 identify 补发。
  try {
    await conversationsApi.stop(kbId, conversationId, generationId);
    // HTTP 只确认收到请求；done 才表示生成结束、部分答案已保存。
  } catch (error) {
    if (lives.get(conversationId) !== slot || !slot.live.streaming) return;
    slot.live = { ...slot.live, stopping: false, stopError: error instanceof Error ? error.message : String(error) };
    flush();
  }
}

// 还没拿到 id 的新会话用内部 token 占位；identify 到真 id 时重映射
let pendingSeq = 0;

export interface LiveHandle {
  /** 新会话从服务端拿到 id：把这个条目从占位 token 重映射到真 id。
   *  `questionId` 是这一问存下的 id，记到最后那条用户消息上 */
  identify: (conversationId: string, questionId?: string, generationId?: string) => void;
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
  finish: (stopped?: boolean) => void;
  /** 请求未被接受时丢弃乐观展示；只断开连接，不取消服务端生成。 */
  discard: () => void;
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
    const slot: Slot = {
      live: { kbId, conversationId, generationId: null, turns, streaming: true, stopping: false },
      abort,
    };
    lives.set(key, slot);
    flush();
    // A follow-up reuses the conversation key. Late callbacks from the old
    // stream must still belong to its original slot, not the replacement.
    const owned = () => (lives.get(key) === slot ? slot : undefined);
    return {
      identify: (id, questionId, generationId) => {
        const current = owned();
        if (!current) return;
        lives.delete(key);
        key = id;
        let turns = current.live.turns;
        if (questionId) {
          const at = turns.map((t) => t.role).lastIndexOf("user");
          if (at >= 0) {
            turns = [...turns];
            turns[at] = { ...turns[at], id: questionId };
          }
        }
        current.live = { ...current.live, conversationId: id, generationId: generationId ?? null, turns };
        lives.set(key, current);
        flush();
        if (current.live.stopping) void requestStop(current);
      },
      patchLast: (f) => {
        const current = owned();
        if (!current?.live.streaming || current.live.turns.length === 0) return;
        const turns = [...current.live.turns];
        turns[turns.length - 1] = f(turns[turns.length - 1]);
        current.live = { ...current.live, turns };
        emit();
      },
      finish: (stopped = false) => {
        const current = owned();
        if (!current || !current.live.streaming) return;
        const turns = stopped
          ? current.live.turns.map((turn, i, all) => i === all.length - 1 ? { ...turn, stopped: true } : turn)
          : current.live.turns;
        current.live = { ...current.live, turns, streaming: false, stopping: false, stopError: undefined };
        flush();
      },
      discard: () => {
        const current = owned();
        if (!current) return;
        current.abort();
        lives.delete(key);
        flush();
      },
      setAbort: (a) => {
        const current = owned();
        if (current) current.abort = a;
      },
    };
  },
  /** 停止按钮专用：请求取消这一代生成，保留连接等服务端收尾。 */
  stop: (kbId: string, conversationId: string | null) => {
    const slot = [...lives.values()].find((s) => s.live.kbId === kbId && s.live.conversationId === conversationId);
    if (!slot?.live.streaming || slot.live.stopping) return;
    slot.live = { ...slot.live, stopping: true, stopError: undefined };
    flush();
    void requestStop(slot);
  },
};
