/* 组那一行与它的子菜单认哪些键（KbMenuList）。与菜单的惯例一致：
   Enter / 空格 / → 打开并进到第一项；Esc / ← 退回那一行；↑↓ 在子菜单里走 */
const OPENS = new Set(["Enter", " ", "ArrowRight"]);
const IN_SUBMENU: Record<string, "close" | 1 | -1> = {
  Escape: "close",
  ArrowLeft: "close",
  ArrowDown: 1,
  ArrowUp: -1,
};

export function groupRowKey(key: string): "open" | undefined {
  return OPENS.has(key) ? "open" : undefined;
}

export function submenuKey(key: string): "close" | 1 | -1 | undefined {
  return IN_SUBMENU[key];
}
