/* 全局消息（toast）：模块级单例 + <ToastHost/>（挂在应用根部一次）。
   任何模块 `import { toast } from "./toast"` 即可弹消息，无需 context 接线。
   右下角堆叠，success/info 3.8s、error 6s 自动消失，可手动关闭。
   可带一个动作（如「撤销」）：带动作的留 8s，点了动作即关闭。 */
import { useEffect, useState } from "react";
import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";

type Kind = "success" | "error" | "info";
type Action = { label: string; onClick: () => void };
type Item = { id: number; kind: Kind; text: string; action?: Action };

let nextId = 1;
let items: Item[] = [];
let listener: ((items: Item[]) => void) | null = null;

function dismiss(id: number) {
  items = items.filter((t) => t.id !== id);
  listener?.(items);
}

function push(kind: Kind, text: string, action?: Action) {
  const id = nextId++;
  items = [...items, { id, kind, text, action }];
  listener?.(items);
  // 带动作的多留一会儿：「撤销」要给人看清楚再点的时间
  const ttl = kind === "error" ? 6000 : action ? 8000 : 3800;
  window.setTimeout(() => dismiss(id), ttl);
}

export const toast = {
  success: (text: string, action?: Action) => push("success", text, action),
  error: (text: string) => push("error", text),
  info: (text: string, action?: Action) => push("info", text, action),
};

const ICON = { success: CheckCircle2, error: AlertCircle, info: Info } as const;
const ICON_COLOR: Record<Kind, string> = {
  success: "text-[var(--u-ok)]",
  error: "text-[var(--u-danger)]",
  info: "text-neutral-400",
};

export function ToastHost() {
  const [list, setList] = useState<Item[]>(items);
  useEffect(() => {
    listener = setList;
    return () => {
      if (listener === setList) listener = null;
    };
  }, []);
  if (!list.length) return null;
  return (
    <div className="pointer-events-none fixed bottom-5 right-5 z-[100] flex flex-col items-end gap-2">
      {list.map((t) => {
        const Icon = ICON[t.kind];
        return (
          <div
            key={t.id}
            className="u-pop u-toast-in pointer-events-auto flex items-center gap-2.5 rounded-xl py-2.5 pl-3.5 pr-2.5 text-sm text-neutral-200 shadow-xl"
          >
            <Icon size={15} className={`shrink-0 ${ICON_COLOR[t.kind]}`} />
            <span className="max-w-xs">{t.text}</span>
            {t.action && (
              <button
                onClick={() => {
                  t.action?.onClick();
                  dismiss(t.id);
                }}
                className="u-link shrink-0 text-xs"
              >
                {t.action.label}
              </button>
            )}
            <button
              onClick={() => dismiss(t.id)}
              className="text-neutral-600 hover:text-neutral-300"
            >
              <X size={13} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
