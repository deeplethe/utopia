/* 库菜单的那一列：顶栏切换器与提问框的作用域芯片共用。名字带 `/` 的库收进
   一个组（`re-cinq ›`），指针停上去或点一下，右边弹出组里的库（#1249）。

   子菜单走 Popover 的 Portal，挂在 body 上——所以它**不在**宿主菜单的 DOM 里。
   宿主若是自己判「点在外面就关」，要放过带 `data-kb-submenu` 的元素，否则
   mousedown 先把整个菜单关掉，点击落空。 */
import { useRef, useState } from "react";
import { Check, ChevronRight } from "lucide-react";
import type { Kb } from "../api";
import { Row } from "../ui";
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/popover";
import { kbGroups, type KbGroupItem } from "./kbGroups";

const ROW = "gap-3 px-4 py-2 text-body";
/** 指针从组那一行移到子菜单要经过一道缝，关得太快就够不着 */
const CLOSE_DELAY_MS = 150;

export function KbMenuList({
  kbs,
  currentId,
  onPick,
  grouped = true,
}: {
  kbs: Kb[];
  currentId: string | undefined;
  onPick: (id: string) => void;
  /** 搜索时传 false：按全名平铺，`re-cinq/lo` 照样搜得到 */
  grouped?: boolean;
}) {
  if (!grouped)
    return kbs.map((k) => (
      <KbRow key={k.id} label={k.name} current={k.id === currentId} onClick={() => onPick(k.id)} />
    ));
  return kbGroups(kbs).map((entry) =>
    entry.kind === "kb" ? (
      <KbRow
        key={entry.kb.id}
        label={entry.kb.name}
        current={entry.kb.id === currentId}
        onClick={() => onPick(entry.kb.id)}
      />
    ) : (
      <KbGroupRow
        key={`group-${entry.name}`}
        name={entry.name}
        items={entry.items}
        currentId={currentId}
        onPick={onPick}
      />
    ),
  );
}

function KbRow({
  label,
  current,
  onClick,
}: {
  label: string;
  current: boolean;
  onClick: () => void;
}) {
  return (
    <Row
      density="menu"
      className={ROW}
      active={current}
      trailing={current ? <Check size={13} className="text-ink-2" /> : undefined}
      onClick={onClick}
    >
      <span className="truncate">{label}</span>
    </Row>
  );
}

function KbGroupRow({
  name,
  items,
  currentId,
  onPick,
}: {
  name: string;
  items: KbGroupItem[];
  currentId: string | undefined;
  onPick: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const closing = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const stay = () => {
    clearTimeout(closing.current);
    setOpen(true);
  };
  const leave = () => {
    clearTimeout(closing.current);
    closing.current = setTimeout(() => setOpen(false), CLOSE_DELAY_MS);
  };
  const holdsCurrent = items.some((i) => i.kb.id === currentId);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverAnchor asChild>
        <div onMouseEnter={stay} onMouseLeave={leave}>
          <Row
            density="menu"
            className={ROW}
            active={open}
            trailing={
              <span className="flex items-center gap-2">
                {holdsCurrent && <Check size={13} className="text-ink-2" />}
                <ChevronRight size={13} className="text-ink-2" />
              </span>
            }
            onClick={() => setOpen((v) => !v)}
          >
            <span className="truncate">{name}</span>
          </Row>
        </div>
      </PopoverAnchor>
      <PopoverContent
        data-kb-submenu
        side="right"
        align="start"
        sideOffset={2}
        collisionPadding={8}
        onOpenAutoFocus={(e) => e.preventDefault()}
        onMouseEnter={stay}
        onMouseLeave={leave}
        className="w-max min-w-48 max-w-80 overflow-hidden p-0"
      >
        <div className="u-scroll max-h-80 overflow-y-auto">
          {items.map((item) => (
            <KbRow
              key={item.kb.id}
              label={item.label}
              current={item.kb.id === currentId}
              onClick={() => {
                setOpen(false);
                onPick(item.kb.id);
              }}
            />
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}
