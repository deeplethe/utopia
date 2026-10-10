/* 主题：暗 / 浅 / 跟系统。**默认暗**——那是这个界面的本色；浅色是给白天对着
 * 它看八小时的人的。存在浏览器里（`utopia.theme`），不经过后端：看的人自己定，
 * 与语言同一个道理（0004）。
 *
 * 落在 `<html data-theme="dark|light">` 上，且**只落这两个值**：「system」在这里
 * 就解成两者之一，CSS 只认 data-theme，不另写一套 prefers-color-scheme 的块——
 * 同一套令牌写两遍迟早分叉。首帧前 index.html 里有一段同样的逻辑，为的是
 * 刷新时不闪一下暗底。 */

export type Theme = "system" | "light" | "dark";
export type Resolved = "light" | "dark";

const KEY = "utopia.theme";
let sessionTheme: Theme | null = null;
const listeners = new Set<(t: Resolved) => void>();

function stored(): Theme {
  if (sessionTheme !== null) return sessionTheme;
  try {
    const v = localStorage.getItem(KEY);
    return v === "light" || v === "dark" || v === "system" ? v : "dark";
  } catch {
    return "dark";
  }
}

export function systemPrefers(): Resolved {
  return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: light)").matches
    ? "light"
    : "dark";
}

export function resolve(t: Theme, system: Resolved = systemPrefers()): Resolved {
  return t === "system" ? system : t;
}

export function getTheme(): Theme {
  return stored();
}

export function resolvedTheme(): Resolved {
  return resolve(stored());
}

function apply(r: Resolved) {
  document.documentElement.dataset.theme = r;
  /* shadcn 的暗色变体认的是 `.dark` 这个类（`@custom-variant dark`），
     我们自己的令牌与画布认的是 `data-theme`。**两个都落**：这样从 shadcn
     registry 里拿来的组件不用改一个字就跟着主题走，而 `data-theme` 那一套
     （画布读令牌、首帧前那段脚本）原样不动 */
  document.documentElement.classList.toggle("dark", r === "dark");
  for (const fn of listeners) fn(r);
}

export function setTheme(t: Theme) {
  try {
    localStorage.setItem(KEY, t);
    sessionTheme = null;
  } catch {
    /* 隐私模式：不存，这一次会话内仍生效 */
    sessionTheme = t;
  }
  apply(resolve(t));
}

/** 主题落定后要重画的东西（画布读的是算出来的令牌，CSS 不会替它刷新）在这里挂 */
export function onThemeChange(fn: (t: Resolved) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** 启动时调一次：把存的选择落到 <html>，并跟着系统偏好变（只在选了 system 时） */
export function initTheme() {
  apply(resolvedTheme());
  if (typeof matchMedia !== "function") return;
  matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => {
    if (stored() === "system") apply(systemPrefers());
  });
}
