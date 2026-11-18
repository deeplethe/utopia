import type { QueryClient } from "@tanstack/react-query";
import { api, type User } from "./api";
import { liveAnswer } from "./liveAnswer";
import { convMarks } from "./unread";
import { kbStore, wsStore } from "./wsStore";

/** 同标签页记忆：上次会话（按库）与未发送草稿——切页回来还原，新标签页从头开始 */
const LAST_PREFIX = "chat:last:";
export const lastKey = (kbId: string) => `${LAST_PREFIX}${kbId}`;
export const DRAFT_KEY = "chat:draft";

/** 这些东西替谁记着。两处各记一份：sessionStorage 里的管这个标签页自己的（草稿、
 *  上次会话、内存里的回答），localStorage 里的管各标签页共用的（未读记号、所选的库）。
 *  只记在 localStorage 不够：别的标签页换了人，共用的那份已经归了新人，这个标签页刷新时
 *  仍要认出自己的草稿是上一个人的。只记在 sessionStorage 也不行：新开的标签页是空的，
 *  同一个人每开一个标签页就要清一次未读和所选的库。 */
const OWNER_KEY = "utopia.account";

/** 这个标签页自己的：回答活得比页面长（liveAnswer），草稿和上次会话活得过刷新 */
function clearTab() {
  liveAnswer.clear();
  sessionStorage.removeItem(DRAFT_KEY);
  // 倒着走：删掉一个键，后面的下标会前移
  for (let i = sessionStorage.length - 1; i >= 0; i--) {
    const key = sessionStorage.key(i);
    if (key?.startsWith(LAST_PREFIX)) sessionStorage.removeItem(key);
  }
}

/** 各标签页共用的：未读记号和所选的库活得过关掉标签页。语言、主题不属于哪个人，不在其中。
 *  要在 clearTab 之后调：摘掉还在写的回答，未读那边会把它们记成"写完了" */
function clearShared() {
  convMarks.clear();
  wsStore.clear();
  kbStore.clear();
}

/** 登进来的是谁，就把东西认到谁名下；记着的不是这个人才清。会话过期后同一个人重新登录，
 *  草稿和上次会话都还在。没记着就当换了人：升级后第一次加载、明确登出之后，留下的东西说不清
 *  是谁的，代价是升级后各清一次。换人不只经过这个页面的登录框：从链接或代理发起的单点登录
 *  在随后取 `me` 时认出来；别的标签页里的登录，在这个标签页下次取 `me` 时（`me` 不过期，
 *  实际就是刷新时）认出来（见 fetchMe） */
export function claimFor(userId: string) {
  if (sessionStorage.getItem(OWNER_KEY) !== userId) clearTab();
  if (localStorage.getItem(OWNER_KEY) !== userId) clearShared();
  sessionStorage.setItem(OWNER_KEY, userId);
  localStorage.setItem(OWNER_KEY, userId);
}

/** `me` 的取数：每次取到都先认一遍。Shell 等 `me` 回来才渲染子页面，
 *  所以 Chat 读草稿之前这一步已经做完 */
export async function fetchMe(): Promise<User> {
  const me = await api.me();
  claimFor(me.id);
  return me;
}

/** 明确登出：这个标签页替这个人记着的全部交出去，查询缓存一起清 */
export function clearSessionState(queryClient: QueryClient) {
  clearTab();
  clearShared();
  sessionStorage.removeItem(OWNER_KEY);
  localStorage.removeItem(OWNER_KEY);
  queryClient.clear();
}
