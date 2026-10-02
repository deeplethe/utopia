import type { QueryClient } from "@tanstack/react-query";
import { liveAnswer } from "./liveAnswer";
import { convMarks } from "./unread";
import { kbStore, wsStore } from "./wsStore";

/** 同标签页记忆：上次会话（按库）与未发送草稿——切页回来还原，新标签页从头开始 */
const LAST_PREFIX = "chat:last:";
export const lastKey = (kbId: string) => `${LAST_PREFIX}${kbId}`;
export const DRAFT_KEY = "chat:draft";

/** 换人时清掉这个标签页替上一个人记着的东西。查询缓存只是其中一份：回答活得比
 *  页面长（liveAnswer），草稿和上次会话活得过刷新（sessionStorage），未读记号和
 *  所选的库还活得过关掉标签页（localStorage）。语言、主题不属于哪个人，留着。 */
export function clearSessionState(queryClient: QueryClient) {
  liveAnswer.clear();
  // 摘掉还在写的回答，未读那边会把它们记成"写完了"，所以未读放在后面清
  convMarks.clear();
  wsStore.clear();
  kbStore.clear();
  sessionStorage.removeItem(DRAFT_KEY);
  // 倒着走：删掉一个键，后面的下标会前移
  for (let i = sessionStorage.length - 1; i >= 0; i--) {
    const key = sessionStorage.key(i);
    if (key?.startsWith(LAST_PREFIX)) sessionStorage.removeItem(key);
  }
  queryClient.clear();
}
