/* 提示：Radix Tooltip。给没有可见文字的东西用（图标按钮、截断的名字、一个数）；
   有可见标签的按钮不需要它。原生 title 要等一秒、样式随系统，这个不。 */
import { Tooltip as RadixTooltip } from "radix-ui";
import type { ReactNode } from "react";

export function Tooltip({
  content,
  side = "top",
  children,
}: {
  content: ReactNode;
  side?: "top" | "bottom" | "left" | "right";
  /** 触发元素；必须能接收 ref 与事件（Button / IconButton / 原生元素都行） */
  children: ReactNode;
}) {
  return (
    <RadixTooltip.Provider delayDuration={300} skipDelayDuration={200}>
      <RadixTooltip.Root>
        <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
        <RadixTooltip.Portal>
          <RadixTooltip.Content
            side={side}
            sideOffset={6}
            collisionPadding={8}
            className="u-pop u-pop-in z-[60] max-w-xs rounded-lg px-2 py-1 text-fine text-ink-2 shadow-xl"
          >
            {content}
          </RadixTooltip.Content>
        </RadixTooltip.Portal>
      </RadixTooltip.Root>
    </RadixTooltip.Provider>
  );
}
