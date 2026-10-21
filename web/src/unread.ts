// 左栏那一列会话此刻各自处在什么状态：正在写 / 写完了你还没看 / 看过了。
//
// **为什么另起一个模块，而不是放进 Chat。** 回答不随离开而停（见 liveAnswer 的
// 开头）：人问完一句话就切去图谱页，Chat 组件卸载，那条流还在跑。"写完了"这个
// 时刻因此常常发生在没有任何组件挂着的时候——判断挂在组件里就等于漏掉它正要
// 记录的那一类事。这里在模块加载时就订上 liveAnswer，此后无人问津也照记。
//
// **"未读"是客户端的事，不进库。** 它说的是"这台机器上的这个人还没看过这一条"，
// 换台机器重新算一遍反而是对的；落进 conversations 表就成了跨设备的一个状态，
// 那是另一件事（要的话得有 read_at 和一条接口）。放 localStorage：刷新还在，
// 清了也只是少一个记号，不丢任何内容。
import { useSyncExternalStore } from "react";
import { liveAnswer } from "./liveAnswer";

const KEY = "utopia.unread";

function load(): string[] {
  try {
    const raw = localStorage.getItem(KEY);
    const v = raw ? JSON.parse(raw) : [];
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

const ids = new Set<string>(load());
const listeners = new Set<() => void>();

/* useSyncExternalStore 靠引用相等跳过重渲染，所以两张表各自留一份快照，
   只在**真的变了**的时候换新对象：一场回答逐字增长期间 liveAnswer 每个
   token 都在 emit，而这两张表在那期间一动不动，左栏不该跟着重画。 */
let unreadSnap: ReadonlySet<string> = new Set(ids);
let liveSnap: ReadonlySet<string> = new Set<string>();

function save() {
  try {
    localStorage.setItem(KEY, JSON.stringify([...ids]));
  } catch {
    /* 隐私模式下存不了：这一会话内照常工作，刷新后从零开始 */
  }
}

function emitUnread() {
  unreadSnap = new Set(ids);
  save();
  for (const l of listeners) l();
}

function same(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  if (a.size !== b.size) return false;
  for (const x of a) if (!b.has(x)) return false;
  return true;
}

/** 此刻正看着的那一场。它写完时不该变成"未读"——人就在那儿看着 */
let viewing: string | null = null;

liveAnswer.subscribe(() => {
  const now = new Set<string>();
  for (const e of liveAnswer.get()) {
    if (e.streaming && e.conversationId) now.add(e.conversationId);
  }
  /* 上一拍在流、这一拍不在了 = 那一场刚写完（正常结束、出错、或人按了停止，
     三者对左栏是同一件事：它不再动了，而你可能没在看）。liveAnswer 的条目
     结束后仍留在表里（只把 streaming 落下来），所以这个差集是可靠的。 */
  let changed = false;
  for (const id of liveSnap) {
    if (now.has(id) || id === viewing) continue;
    if (!ids.has(id)) {
      ids.add(id);
      changed = true;
    }
  }
  if (!same(now, liveSnap)) {
    liveSnap = now;
    if (!changed) for (const l of listeners) l();
  }
  if (changed) emitUnread();
});

export const convMarks = {
  /** 换人时清空：这些都是上一个人的会话。要在 liveAnswer.clear 之后调——摘掉
   *  还在写的条目时，上面的订阅会把它们记成写完未读。liveSnap 不在这里清：它跟着
   *  liveAnswer 走，回答清了它自然空；只清共用那份时，留下的回答还要接着记 */
  clear() {
    ids.clear();
    viewing = null;
    unreadSnap = new Set();
    try {
      localStorage.removeItem(KEY);
    } catch {
      /* 同 save：存不了就只清内存 */
    }
    for (const l of listeners) l();
  },
  unread: (): ReadonlySet<string> => unreadSnap,
  live: (): ReadonlySet<string> => liveSnap,
  subscribe(l: () => void) {
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  },
  /** 打开了某一场（传 null = 谁也没打开，比如新对话）。打开就是看过了 */
  view(conversationId: string | null) {
    viewing = conversationId;
    if (conversationId && ids.delete(conversationId)) emitUnread();
  },
  /** 会话删了，记号跟着走——否则这个 id 永远留在存储里 */
  forget(conversationId: string) {
    if (ids.delete(conversationId)) emitUnread();
  },
};

/** 有未读的那些会话 */
export function useUnread(): ReadonlySet<string> {
  return useSyncExternalStore(convMarks.subscribe, convMarks.unread);
}

/** 此刻正在写的那些会话 */
export function useLive(): ReadonlySet<string> {
  return useSyncExternalStore(convMarks.subscribe, convMarks.live);
}
