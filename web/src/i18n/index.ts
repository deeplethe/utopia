// 界面语言：**看的人自己定**，不经过后端（见 docs/decisions/0004）。
// 和暗色模式同类——并发上限那种设置描述的是部署，界面语言描述的是读者。
//
// locale 只在这一个文件里解析。其余 600 多处只消费 `S`，谁都不去读来源。
// 将来要不要跟随浏览器、要不要每用户覆盖，改的是这里，不是那 600 处。
import { en, type Strings } from "./en";
import { zh } from "./zh";

export type { Strings };

export const LANGS = ["en", "zh"] as const;
export type Lang = (typeof LANGS)[number];

/** 语言自己的名字，永远不翻译——在切换器里，"中文"对看不懂英文的人才是路标 */
export const LANG_NAMES: Record<Lang, string> = { en: "English", zh: "中文" };

const BUNDLES: Record<Lang, Strings> = { en, zh };
const KEY = "utopia.lang";

function detect(): Lang {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved && (LANGS as readonly string[]).includes(saved))
      return saved as Lang;
  } catch {
    // 隐私模式下 localStorage 会抛——回落到英文，别让首屏挂掉
  }
  // **不跟随浏览器语言**。中文包还在跟着英文包后面追，猜错语言的代价是
  // 一个中文用户看到半成品，而不是看到完整的英文。等 zh 追平了再把
  // navigator.language 那一句加回来——那是一行代码的事。
  // 人自己选过的仍然作数（上面那段），切换器照常在用户菜单里
  return "en";
}

export const lang: Lang = detect();

/** 当前语言包。模块加载时定一次——所以模块级常量（`const X = S.a.b`）也是对的 */
export const S: Strings = BUNDLES[lang];

/**
 * 切语言走整页重载，不是重挂载。
 *
 * 635 处引用里已经有一处在模块顶层求值（`Members.tsx` 的 ROLE_OPTIONS），
 * 而这种写法今后还会有人写。重挂载会让它们停在旧语言且**不报错**；
 * 重载让整个模块图重新求值，代价只是一次刷新——切语言一年也没几次。
 */
export function setLang(next: Lang) {
  if (next === lang) return;
  try {
    localStorage.setItem(KEY, next);
  } catch {
    // 存不下就只对本次会话生效，好过什么都不发生
  }
  location.reload();
}
