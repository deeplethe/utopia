/* 引用：角标、落款行，和它们共用的预览浮窗。
   点一条引用先给一段原文，右上角才是去原文的路——核对一句话多半只要看那一段，
   而跳转会把人从答案里带走，回来时答案已经滚上去了。 */
import { Popover as RadixPopover } from "radix-ui";
import type { ReactNode } from "react";

/** 正文里的一个 `[n]`。流式中来源还没到的号用 {@link CiteMark} 画成静的 */
export function CiteChip({
  n,
  preview,
}: {
  n: number;
  /** 浮窗内容；只有点开才挂载，所以里面的请求也是点开才发 */
  preview: ReactNode;
}) {
  return (
    <RadixPopover.Root>
      <RadixPopover.Trigger asChild>
        <button
          type="button"
          className="u-num rounded-cell px-1 text-accent transition-colors duration-fast hover:bg-surface-2"
        >
          [{n}]
        </button>
      </RadixPopover.Trigger>
      <PreviewPortal>{preview}</PreviewPortal>
    </RadixPopover.Root>
  );
}

/** 还点不动的号（来源尚未到达）。占同样的位置，只是不发光 */
export function CiteMark({ n }: { n: number }) {
  return <span className="u-num px-1 text-ink-2">[{n}]</span>;
}

/** 落款里的一行。整行是触发器，点开同一个浮窗 */
export function CiteRow({
  children,
  preview,
}: {
  children: ReactNode;
  preview: ReactNode;
}) {
  return (
    <RadixPopover.Root>
      <RadixPopover.Trigger asChild>
        <button
          type="button"
          className="block w-full px-3 py-2 text-left text-small text-ink-2 transition-colors duration-fast hover:bg-surface-2"
        >
          {children}
        </button>
      </RadixPopover.Trigger>
      <PreviewPortal align="start">{preview}</PreviewPortal>
    </RadixPopover.Root>
  );
}

function PreviewPortal({
  children,
  align = "center",
}: {
  children: ReactNode;
  align?: "start" | "center";
}) {
  return (
    <RadixPopover.Portal>
      <RadixPopover.Content
        side="top"
        align={align}
        sideOffset={6}
        collisionPadding={8}
        className="u-pop u-pop-in z-[60] rounded-panel u-lift"
      >
        {children}
      </RadixPopover.Content>
    </RadixPopover.Portal>
  );
}

/** 浮窗的样子：标题一行、原文一段、右上角一条出路 */
export function PreviewCard({
  title,
  meta,
  body,
  action,
}: {
  title: ReactNode;
  /** 标题下面的一行小字（章节名、这段文字的来历） */
  meta?: ReactNode;
  body: ReactNode;
  /** 去原文的链接，放在标题右边 */
  action: ReactNode;
}) {
  return (
    <div className="w-80 max-w-[min(24rem,90vw)]">
      <div className="flex items-start gap-2 border-b border-line px-3 py-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-small text-ink">{title}</div>
          {meta && <div className="truncate text-fine text-ink-2">{meta}</div>}
        </div>
        <div className="shrink-0">{action}</div>
      </div>
      {/* 原文按原样排（换行是它的一部分），长了自己滚，别把浮窗撑穿屏幕 */}
      <div className="max-h-64 overflow-auto whitespace-pre-wrap px-3 py-2 text-small leading-relaxed text-ink-2">
        {body}
      </div>
    </div>
  );
}
