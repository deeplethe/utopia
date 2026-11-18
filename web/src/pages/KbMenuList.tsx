/* 库菜单的那一列：顶栏切换器与提问框的作用域芯片共用。名字带 `/` 的库收进
   一个组（`re-cinq ›`），指针停上去或点一下，右边弹出组里的库（#1249）。

   子菜单走 Popover 的 Portal，挂在 body 上——所以它**不在**宿主菜单的 DOM 里。
   宿主若是自己判「点在外面就关」，要放过带 `data-kb-submenu` 的元素，否则
   mousedown 先把整个菜单关掉，点击落空。

   键盘：组那一行上 Enter / 空格 / → 打开并把焦点送进第一项，↑↓ 在子菜单里走，
   Esc / ← 关上并把焦点还给那一行（kbMenuKeys）。没用 DropdownMenuSub：它只能
   长在 DropdownMenu 里，而两个宿主一个是带查找框的 Popover（菜单的 typeahead
   与焦点管理会跟查找框抢键），一个是手写的 div。 */
import { useEffect, useRef, useState, useSyncExternalStore, type KeyboardEvent } from "react";
import { Check, ChevronRight } from "lucide-react";
import type { Kb } from "../api";
import { Row, cn } from "../ui";
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/popover";
import { kbGroups, type KbGroupItem } from "./kbGroups";
import { groupRowKey, submenuKey } from "./kbMenuKeys";

const ROW = "gap-3 px-4 py-2 text-body";
/** 指针从组那一行移到子菜单要经过一道缝，关得太快就够不着 */
const CLOSE_DELAY_MS = 150;

const NARROW = "(max-width: 639px)";

/** 屏幕窄到放不下侧边子菜单（手机竖屏） */
function useNarrow() {
  return useSyncExternalStore(
    (changed) => {
      const query = window.matchMedia(NARROW);
      query.addEventListener("change", changed);
      return () => query.removeEventListener("change", changed);
    },
    () => window.matchMedia(NARROW).matches,
  );
}

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
  indent,
}: {
  label: string;
  current: boolean;
  onClick: () => void;
  indent?: number;
}) {
  return (
    /* 对勾走 `trailing` 槽。**塞进 children 的 svg 会掉到第二行**：
       Row 把 children 整个包进一个 `flex-1 truncate` 的 span，
       那个 span 不是 flex 容器，preflight 又把 svg 设成块级，
       于是勾自己占一行、整行跟着变高 */
    <Row
      density="menu"
      className={ROW}
      active={current}
      indent={indent}
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
  const narrow = useNarrow();
  const closing = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const anchor = useRef<HTMLDivElement>(null);
  const submenu = useRef<HTMLDivElement>(null);
  /** 键盘打开的才把焦点送进子菜单；指针停上去的不抢（顶栏的查找框还在打字） */
  const focusInside = useRef(false);
  const show = (withFocus: boolean) => {
    clearTimeout(closing.current);
    focusInside.current = withFocus;
    setOpen(true);
  };
  const leave = () => {
    clearTimeout(closing.current);
    closing.current = setTimeout(() => setOpen(false), CLOSE_DELAY_MS);
  };
  /* 只有鼠标按悬停开合。触屏的一点会先模拟一次 enter 再来 click——
     若 click 是开关，就刚开就关 */
  const byMouse = (run: () => void) => (e: { pointerType: string }) => {
    if (e.pointerType === "mouse") run();
  };
  const onRowKey = (e: KeyboardEvent) => {
    if (groupRowKey(e.key) !== "open") return;
    e.preventDefault();
    // 指针已经把它打开了：onOpenAutoFocus 不会再来，焦点自己送进去
    if (open) submenu.current?.querySelector("button")?.focus();
    else show(true);
  };
  useEffect(() => {
    if (!narrow || !open || !focusInside.current) return;
    submenu.current?.querySelector("button")?.focus();
    focusInside.current = false;
  }, [narrow, open]);
  const onSubmenuKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const action = submenuKey(e.key);
    if (action === undefined) return;
    e.preventDefault();
    // 别让提问框那个宿主的 Esc（挂在 document 上）把整个菜单也关了
    e.stopPropagation();
    if (action === "close") {
      setOpen(false);
      anchor.current?.querySelector("button")?.focus();
      return;
    }
    const buttons = [...e.currentTarget.querySelectorAll("button")];
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    buttons[(at + action + buttons.length) % buttons.length]?.focus();
  };
  const holdsCurrent = items.some((i) => i.kb.id === currentId);
  const trailing = (
    <span className="flex items-center gap-2">
      {holdsCurrent && <Check size={13} className="text-ink-2" />}
      <ChevronRight
        size={13}
        className={cn("u-turn text-ink-2", narrow && open && "rotate-90")}
      />
    </span>
  );
  /* 窄屏（手机）上菜单几乎占满宽度，右边左边都放不下子菜单——Radix 只好把它
     推到屏幕外。这里改成就地展开：组下面缩进一档列出它的库 */
  if (narrow)
    return (
      <>
        <div ref={anchor}>
          <Row
            density="menu"
            className={ROW}
            aria-expanded={open}
            trailing={trailing}
            onClick={() => setOpen((v) => !v)}
            onKeyDown={onRowKey}
          >
            <span className="truncate">{name}</span>
          </Row>
        </div>
        {open &&
          <div ref={submenu} onKeyDown={onSubmenuKey}>
            {items.map((item) => (
              <KbRow
                key={item.kb.id}
                label={item.label}
                indent={1}
                current={item.kb.id === currentId}
                onClick={() => {
                  setOpen(false);
                  onPick(item.kb.id);
                }}
              />
            ))}
          </div>
        }
      </>
    );
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverAnchor asChild>
        <div ref={anchor} onPointerEnter={byMouse(() => show(false))} onPointerLeave={byMouse(leave)}>
          <Row
            density="menu"
            className={ROW}
            aria-haspopup="menu"
            aria-expanded={open}
            trailing={trailing}
            onClick={() => show(false)}
            onKeyDown={onRowKey}
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
        onOpenAutoFocus={(e) => {
          if (!focusInside.current) e.preventDefault();
        }}
        // 焦点在子菜单里时 Esc 交给 onSubmenuKey（它还要把焦点还给那一行）；
        // 焦点还在搜索框里时照常关掉
        onEscapeKeyDown={(e) => {
          if (submenu.current?.contains(document.activeElement)) e.preventDefault();
        }}
        // 按在组那一行上不算外面：否则按下就关、松开的 click 又开，闪一下
        onInteractOutside={(e) => {
          if (anchor.current?.contains(e.target as Node)) e.preventDefault();
        }}
        onKeyDown={onSubmenuKey}
        onPointerEnter={byMouse(() => show(false))}
        onPointerLeave={byMouse(leave)}
        className="w-max min-w-48 max-w-80 overflow-hidden p-0"
      >
        <div ref={submenu} className="u-scroll max-h-80 overflow-y-auto">
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
