/** 把一段文字放进剪贴板，交回放没放成。
 *
 *  `navigator.clipboard` 只在安全上下文里有：https，或者 localhost。在局域网里用
 *  http 打开的部署上它是 undefined——自托管最常见的那种打开方式——这时候直接调它，
 *  按钮按了什么也不发生，也不报错。那里退回 `execCommand("copy")`：选中一个看不见的
 *  textarea 再复制。这个接口已经不推荐了，但浏览器都还认，而且它只在剪贴板 API
 *  用不了的时候才走到。两样都不成就交回 false，由调用方说出来，不静默。 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // 权限被拒、页面没有焦点：走下面的老办法
  }
  const focused = document.activeElement as HTMLElement | null;
  const area = document.createElement("textarea");
  area.value = text;
  // 只读：手机上不弹键盘；固定在视口里、透明：不把页面滚走，也看不见
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.top = "0";
  area.style.left = "0";
  area.style.opacity = "0";
  document.body.appendChild(area);
  try {
    area.focus();
    area.select();
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    area.remove();
    focused?.focus?.();
  }
}
