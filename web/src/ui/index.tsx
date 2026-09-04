/* Utopia UI 组件库 — 页面只用这里的组件与 styles.css 语义类，不写颜色字面量。 */
import { useEffect, useRef, useState } from "react";
import type {
  ButtonHTMLAttributes,
  InputHTMLAttributes,
  ReactNode,
} from "react";
import {
  ArrowUpRight,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Search as SearchIcon,
} from "lucide-react";
import { S } from "../i18n";

/** 应用内左栏统一底座：宽度 + 玻璃面（各页在此之上加 flex/padding）。
    以最宽的 Ontology（w-64）为基准——rail 装的是名字，宽一档少截断。 */
export const RAIL_CLS = "w-64 shrink-0 glass-strong border-y-0 border-l-0";

/** 品牌字标：Marcellus 衬线，逐字母从左到右淡入；hover 浮出 ↗，点击去官网。
    箭头/偏移全部用 em，跟随使用处的字号缩放（登录大标题与顶栏共用）。 */
export function Wordmark({ className }: { className?: string }) {
  return (
    <a
      href={S.app.siteUrl}
      target="_blank"
      rel="noreferrer"
      title="utopia.bi"
      className={cn("relative inline-flex text-white", className)}
      style={{ fontFamily: "var(--font-brand)", letterSpacing: "0.06em" }}
    >
      {[...S.app.name].map((ch, i) => (
        <span
          key={i}
          className="u-letter"
          style={{ animationDelay: `${80 + i * 65}ms` }}
        >
          {ch}
        </span>
      ))}
      <ArrowUpRight className="u-mark-arrow" aria-hidden />
    </a>
  );
}

export function cn(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

/* ---------- Button ---------- */
type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "ghost";
  size?: "sm" | "md";
};

export function Button({
  variant = "primary",
  size = "md",
  className,
  ...props
}: ButtonProps) {
  return (
    <button
      className={cn(
        "u-btn",
        variant === "primary" ? "u-btn-primary" : "u-btn-ghost",
        size === "sm" ? "px-3 py-1.5 text-xs" : "px-4 py-2 text-sm",
        className,
      )}
      {...props}
    />
  );
}

/* ---------- Input / Select ---------- */
export function Input({
  className,
  ...props
}: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn("input-dark px-3 py-2 text-sm", className)}
      {...props}
    />
  );
}

/* ---------- Dropdown（自制下拉，替代原生 select：原生弹层无法主题化） ---------- */
export interface DropdownOption {
  value: string;
  label: ReactNode;
}

export function Dropdown({
  value,
  options,
  onChange,
  placeholder,
  className,
  size = "md",
  icon,
  menuLabel,
  footer,
}: {
  value: string;
  options: DropdownOption[];
  onChange: (v: string) => void;
  placeholder?: string;
  className?: string;
  size?: "sm" | "md";
  /** 触发器左侧的语义图标（说明"这一级是什么"） */
  icon?: ReactNode;
  /** 弹层顶部的小标题（同时作为触发器 title 提示） */
  menuLabel?: string;
  /** 弹层底部固定操作区（点击后弹层关闭） */
  footer?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const current = options.find((o) => o.value === value);
  const pad = size === "sm" ? "px-2.5 py-1 text-xs" : "px-3 py-1.5 text-sm";

  return (
    <div ref={rootRef} className={cn("relative", className)}>
      <button
        type="button"
        onClick={() => setOpen(!open)}
        title={menuLabel}
        className={cn(
          "input-dark w-full flex items-center gap-2 text-left",
          pad,
        )}
      >
        {icon && <span className="shrink-0 text-neutral-500">{icon}</span>}
        <span className="flex-1 min-w-0 truncate">
          {current?.label ?? (
            <span className="text-neutral-600">{placeholder ?? ""}</span>
          )}
        </span>
        <ChevronDown
          size={12}
          className={cn(
            "shrink-0 text-neutral-500 transition-transform",
            open && "rotate-180",
          )}
        />
      </button>
      {open && (
        <div className="u-pop u-pop-in u-pop-in-tl absolute z-50 mt-1 w-full rounded-lg shadow-xl overflow-hidden">
          {menuLabel && (
            <div className="px-2.5 pt-2 pb-1 text-[9.5px] font-medium uppercase tracking-[0.1em] text-neutral-600 border-b border-white/5">
              {menuLabel}
            </div>
          )}
          {/* 选项行顶满面板边缘（无内衬）：单选项时整个菜单被这一项填满 */}
          <div className="u-scroll max-h-60 overflow-y-auto">
            {options.map((o) => (
              <button
                key={o.value}
                type="button"
                onClick={() => {
                  onChange(o.value);
                  setOpen(false);
                }}
                className={cn(
                  "w-full flex items-center gap-2 text-left",
                  pad,
                  o.value === value
                    ? "bg-white/[0.12] text-white"
                    : "text-neutral-300 hover:bg-white/[0.06] hover:text-white",
                )}
              >
                <span className="flex-1 min-w-0 truncate">{o.label}</span>
                {o.value === value && (
                  <Check size={12} className="shrink-0 text-neutral-400" />
                )}
              </button>
            ))}
          </div>
          {footer && (
            <div
              className="border-t border-white/10"
              onClick={() => setOpen(false)}
            >
              {footer}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/* ---------- SearchSelect（可搜索选择器：无界对象列表专用——成员、父类、数据源…）
   触发器本身是输入框：聚焦即开、键入即过滤；渲染上限 maxVisible，超出提示继续
   输入收窄。小而有界的枚举（角色/数据类型…）仍用 Dropdown，两击即达不必打字。 ---------- */
export interface SearchSelectOption {
  value: string;
  /** 主文案：过滤与选中回显的依据（纯字符串，不能是节点） */
  label: string;
  /** 次要文案（邮箱、连接摘要…），一并参与过滤，弱化显示 */
  hint?: string;
  /** 层级缩进（浏览态展示树形；键入过滤后拉平对齐） */
  indent?: number;
}

export function SearchSelect({
  value,
  options,
  onChange,
  placeholder,
  className,
  size = "md",
  maxVisible = 8,
}: {
  value: string;
  options: SearchSelectOption[];
  onChange: (v: string) => void;
  placeholder?: string;
  className?: string;
  size?: "sm" | "md";
  maxVisible?: number;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const current = options.find((o) => o.value === value);
  const q = query.trim().toLowerCase();
  const matches = q
    ? options.filter((o) =>
        `${o.label} ${o.hint ?? ""}`.toLowerCase().includes(q),
      )
    : options;
  const visible = matches.slice(0, maxVisible);
  const hidden = matches.length - visible.length;

  const pick = (v: string) => {
    onChange(v);
    setOpen(false);
    setQuery("");
    inputRef.current?.blur();
  };

  const pad =
    size === "sm" ? "pl-7 pr-2.5 py-1 text-xs" : "pl-8 pr-3 py-1.5 text-sm";
  const rowPad = size === "sm" ? "px-2.5 py-1 text-xs" : "px-3 py-1.5 text-sm";

  return (
    <div className={cn("relative", className)}>
      <SearchIcon
        size={size === "sm" ? 11 : 13}
        className="absolute left-2.5 top-1/2 -translate-y-1/2 text-neutral-600 pointer-events-none"
      />
      <input
        ref={inputRef}
        className={cn("input-dark w-full", pad)}
        value={open ? query : (current?.label ?? "")}
        /* 打开后把当前选中项挪进 placeholder：边打字边能看到现值 */
        placeholder={open ? current?.label || placeholder : placeholder}
        onFocus={() => {
          setOpen(true);
          setQuery("");
          setActive(0);
        }}
        /* 选项行 mousedown 已 preventDefault（不夺焦点），走到这里的失焦都是真离开 */
        onBlur={() => setOpen(false)}
        onChange={(e) => {
          setQuery(e.target.value);
          setActive(0);
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            setOpen(false);
            inputRef.current?.blur();
          } else if (e.key === "ArrowDown") {
            e.preventDefault();
            setActive((a) => Math.min(a + 1, visible.length - 1));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setActive((a) => Math.max(a - 1, 0));
          } else if (e.key === "Enter" && visible[active]) {
            e.preventDefault();
            pick(visible[active].value);
          }
        }}
      />
      {open && (
        <div className="u-pop u-pop-in u-pop-in-tl absolute z-50 mt-1 w-full rounded-lg shadow-xl overflow-hidden">
          {visible.map((o, i) => (
            <button
              key={o.value}
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => pick(o.value)}
              onMouseEnter={() => setActive(i)}
              className={cn(
                "w-full flex items-center gap-2 text-left",
                rowPad,
                i === active
                  ? "bg-white/[0.08] text-white"
                  : "text-neutral-300",
              )}
            >
              {!q && !!o.indent && (
                <span className="shrink-0" style={{ width: o.indent * 14 }} />
              )}
              <span className="min-w-0 flex-1 truncate">
                {o.label}
                {o.hint && (
                  <span className="ml-2 text-neutral-500">{o.hint}</span>
                )}
              </span>
              {o.value === value && (
                <Check size={12} className="shrink-0 text-neutral-400" />
              )}
            </button>
          ))}
          {visible.length === 0 && (
            <p className={cn(rowPad, "text-neutral-600")}>{S.ui.noMatches}</p>
          )}
          {hidden > 0 && (
            <div
              className={cn(
                rowPad,
                "border-t border-white/5 text-[11px] text-neutral-600",
              )}
            >
              {S.ui.keepTyping(hidden)}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/* ---------- MultiSearchSelect（多选版 SearchSelect） ---------- */

/**
 * 多选 + 搜索。与 [`SearchSelect`] 同一套语汇与键盘操作，三处不同：
 *
 * - **已选的显示在输入框上方**，各带一个移除按钮。不显示在下拉里的原因是
 *   下拉一关就看不见了，而"我到底选了哪些"是随时要看的
 * - **选完不关**：多选多半要连点几个，每次都重新聚焦是折磨
 * - 已选项在列表里带勾，再点一次是取消
 *
 * 选项多到几百个时（大本体就是这个量级）它仍然可用——这正是它取代芯片墙的理由：
 * 芯片墙的高度随类数线性增长，搜索框不随。
 */
export function MultiSearchSelect({
  values,
  options,
  onToggle,
  placeholder,
  emptyHint,
  className,
  maxVisible = 8,
}: {
  values: string[];
  options: SearchSelectOption[];
  onToggle: (v: string) => void;
  placeholder?: string;
  /** 一个都没选时显示的话。多选留空往往是有意义的（"不限"），不是没填 */
  emptyHint?: string;
  className?: string;
  maxVisible?: number;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const q = query.trim().toLowerCase();
  const matches = q
    ? options.filter((o) =>
        `${o.label} ${o.hint ?? ""}`.toLowerCase().includes(q),
      )
    : options;
  const visible = matches.slice(0, maxVisible);
  const hidden = matches.length - visible.length;
  const picked = values
    .map((v) => options.find((o) => o.value === v))
    .filter((o): o is SearchSelectOption => !!o);

  const toggle = (v: string) => {
    onToggle(v);
    setQuery("");
    setActive(0);
    inputRef.current?.focus();
  };

  return (
    <div className={cn("relative", className)}>
      {picked.length > 0 && (
        <div className="mb-1 flex flex-wrap gap-1">
          {picked.map((o) => (
            <button
              key={o.value}
              type="button"
              onClick={() => onToggle(o.value)}
              className="group flex items-center gap-1 rounded-full bg-white/[0.10] px-2 py-0.5 text-[11px] text-neutral-200 hover:bg-white/[0.16] transition-colors"
              title={o.hint ?? o.label}
            >
              {o.label}
              <span className="text-neutral-500 group-hover:text-neutral-200">
                ✕
              </span>
            </button>
          ))}
        </div>
      )}
      {picked.length === 0 && emptyHint && (
        <p className="mb-1 text-[11px] text-neutral-600">{emptyHint}</p>
      )}
      <SearchIcon
        size={11}
        className="absolute left-2.5 top-1/2 -translate-y-1/2 text-neutral-600 pointer-events-none"
        style={{ top: undefined }}
      />
      <input
        ref={inputRef}
        className="input-dark w-full pl-7 pr-2.5 py-1 text-xs"
        value={query}
        placeholder={placeholder}
        onFocus={() => {
          setOpen(true);
          setActive(0);
        }}
        onBlur={() => setOpen(false)}
        onChange={(e) => {
          setQuery(e.target.value);
          setActive(0);
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            setOpen(false);
            inputRef.current?.blur();
          } else if (e.key === "ArrowDown") {
            e.preventDefault();
            setActive((a) => Math.min(a + 1, visible.length - 1));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setActive((a) => Math.max(a - 1, 0));
          } else if (e.key === "Enter" && visible[active]) {
            e.preventDefault();
            toggle(visible[active].value);
          } else if (e.key === "Backspace" && !query && picked.length) {
            // 空输入时退格删掉最后一个 —— 与各家 token 输入框一致
            onToggle(picked[picked.length - 1].value);
          }
        }}
      />
      {open && (
        <div className="u-pop u-pop-in u-pop-in-tl absolute z-50 mt-1 w-full rounded-lg shadow-xl overflow-hidden">
          {visible.map((o, i) => (
            <button
              key={o.value}
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => toggle(o.value)}
              onMouseEnter={() => setActive(i)}
              className={cn(
                "w-full flex items-center gap-2 text-left px-2.5 py-1 text-xs",
                i === active
                  ? "bg-white/[0.08] text-white"
                  : "text-neutral-300",
              )}
            >
              {!q && !!o.indent && (
                <span className="shrink-0" style={{ width: o.indent * 14 }} />
              )}
              <span className="min-w-0 flex-1 truncate">
                {o.label}
                {o.hint && (
                  <span className="ml-2 text-neutral-500">{o.hint}</span>
                )}
              </span>
              {values.includes(o.value) && (
                <Check size={12} className="shrink-0 text-neutral-400" />
              )}
            </button>
          ))}
          {visible.length === 0 && (
            <p className="px-2.5 py-1 text-xs text-neutral-600">
              {S.ui.noMatches}
            </p>
          )}
          {hidden > 0 && (
            <div className="px-2.5 py-1 text-[11px] text-neutral-600 border-t border-white/5">
              {S.ui.keepTyping(hidden)}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/* ---------- ColorPicker（精选色板 + hex 兜底；实体色刻意不开放全色域） ----------
   **改这里就得改 `crates/utopia-store/src/palette.rs`**：手动挑的色与自动按 key
   取的色必须来自同一组，否则一张图里会出现两套配色。那边有测试盯着，改漏了会红。 */
export const ENTITY_PALETTE = [
  "#7fd0ff",
  "#5fa8ff",
  "#5fd4d0",
  "#63e2b7",
  "#4cc38a",
  "#a8d878",
  "#ffd479",
  "#f2b66d",
  "#ff9d76",
  "#ff8a9e",
  "#ff9daf",
  "#e797d8",
  "#c4a5ff",
  "#9fa8ff",
  "#8ea5bd",
  "#b3b9c4",
];

/**
 * 类的 key → 颜色。**必须与 `crates/utopia-store/src/palette.rs` 的
 * `color_for_key` 逐位一致**：新建类时前端先按 key 挑一个显示出来，
 * 用户不改就这么存下去；而导入/消解那条路是后端算的。两边算得不一样，
 * 同一个 key 就会因为「谁建的」而拿到不同颜色。
 *
 * FNV-1a + 雪崩混合。用 BigInt 是因为 JS 的位运算是 32 位的，
 * 而这里要的是 64 位乘法——用 Number 做会静默丢高位，
 * 算出来跟 Rust 对不上，且不会有任何报错。
 */
export function colorForKey(key: string): string {
  let h = 0xcbf29ce484222325n;
  const M = (1n << 64n) - 1n;
  for (const b of new TextEncoder().encode(key)) {
    h = (h ^ BigInt(b)) & M;
    h = (h * 0x100000001b3n) & M;
  }
  h = (h ^ (h >> 33n)) & M;
  h = (h * 0xff51afd7ed558ccdn) & M;
  h = (h ^ (h >> 33n)) & M;
  return ENTITY_PALETTE[Number(h % BigInt(ENTITY_PALETTE.length))];
}

export function ColorPicker({
  value,
  onChange,
  shape,
}: {
  value: string;
  onChange: (v: string) => void;
  /** 给定时，色井渲染"形状 + 颜色"而不是整块填充（方形是直角，与图谱节点一致） */
  shape?: "circle" | "square";
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const valid = /^#[0-9a-fA-F]{6}$/.test(value);

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative inline-block">
      {/* 触发器：当前颜色色块（Figma 式 color well）；带 shape 时渲染形状 + 颜色 */}
      {shape ? (
        <button
          type="button"
          title={value}
          onClick={() => setOpen(!open)}
          className="h-8 w-14 rounded-lg border border-white/15 hover:border-white/35 transition-colors bg-white/[0.04] grid place-items-center"
        >
          <span
            className={cn("h-3.5 w-3.5", shape === "circle" && "rounded-full")}
            style={{ background: valid ? value : ENTITY_PALETTE[0] }}
          />
        </button>
      ) : (
        <button
          type="button"
          title={value}
          onClick={() => setOpen(!open)}
          className="h-8 w-14 rounded-lg border border-white/15 hover:border-white/35 transition-colors"
          style={{ background: valid ? value : ENTITY_PALETTE[0] }}
        />
      )}
      {open && (
        // 显式宽度：绝对定位的收缩宽度会被 inline-block 触发器的 56px 容器块钳死
        <div className="u-pop u-pop-in u-pop-in-tl absolute z-50 left-0 top-full mt-2 w-56 rounded-xl p-3 shadow-xl">
          <div className="grid grid-cols-8 gap-1.5 mb-2.5">
            {ENTITY_PALETTE.map((c) => (
              <button
                key={c}
                type="button"
                title={c}
                onClick={() => {
                  onChange(c);
                  setOpen(false);
                }}
                className={cn(
                  "h-5 w-5 rounded-full transition-transform hover:scale-110",
                  value.toLowerCase() === c &&
                    "outline outline-2 outline-white/80 outline-offset-1",
                )}
                style={{ background: c }}
              />
            ))}
          </div>
          <input
            value={value}
            onChange={(e) => onChange(e.target.value)}
            placeholder={ENTITY_PALETTE[0]}
            className={cn(
              "input-dark w-full px-2 py-1 text-xs font-mono",
              !valid && "!border-[var(--u-danger)]",
            )}
          />
        </div>
      )}
    </div>
  );
}

/* ---------- Pager（列表分页条：不足一页时自动隐藏） ---------- */
export function Pager({
  total,
  pageSize,
  page,
  onPage,
  /** 覆盖默认的上边距。默认 `mt-3` 适合跟在列表后面；
      放进一个已经有内边距的底栏时传 `""` 去掉它 */
  className = "mt-3",
}: {
  total: number;
  pageSize: number;
  page: number;
  onPage: (p: number) => void;
  className?: string;
}) {
  const pageCount = Math.max(1, Math.ceil(total / pageSize));
  const safe = Math.min(page, pageCount - 1);
  if (total <= pageSize) return null;
  return (
    <div className={cn("flex items-center justify-end gap-2 text-xs text-neutral-500", className)}>
      <span className="u-num">
        {S.library.pageOf(
          safe * pageSize + 1,
          Math.min((safe + 1) * pageSize, total),
          total,
        )}
      </span>
      <button
        onClick={() => onPage(safe - 1)}
        disabled={safe === 0}
        className="u-btn u-btn-ghost h-7 w-7 grid place-items-center rounded-lg"
      >
        <ChevronLeft size={13} />
      </button>
      <button
        onClick={() => onPage(safe + 1)}
        disabled={safe >= pageCount - 1}
        className="u-btn u-btn-ghost h-7 w-7 grid place-items-center rounded-lg"
      >
        <ChevronRight size={13} />
      </button>
    </div>
  );
}

/** 分页切片辅助：返回当前页数据与安全页号。 */
export function pageSlice<T>(
  items: T[],
  page: number,
  pageSize: number,
): { rows: T[]; safe: number } {
  const pageCount = Math.max(1, Math.ceil(items.length / pageSize));
  const safe = Math.min(page, pageCount - 1);
  return { rows: items.slice(safe * pageSize, (safe + 1) * pageSize), safe };
}

/* ---------- DangerConfirm（危险操作确认弹窗：可要求输入指定文本解锁） ---------- */
export function DangerConfirm({
  title,
  hint,
  requireText,
  confirmLabel,
  cancelLabel,
  busy,
  onConfirm,
  onCancel,
}: {
  title: string;
  hint: string;
  /** 要求逐字输入的解锁文本（如资源名称）；缺省则直接可确认 */
  requireText?: string;
  confirmLabel: string;
  cancelLabel: string;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState("");
  const unlocked = !requireText || text === requireText;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div
      className="u-modal-scrim fixed inset-0 z-50 grid place-items-center bg-black/80 backdrop-blur-sm"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="u-modal-panel u-modal-in w-[24rem] max-w-[calc(100vw-2rem)] rounded-2xl shadow-2xl p-5">
        <h2 className="text-[15px] font-semibold text-[var(--u-danger)] mb-2">
          {title}
        </h2>
        <p className="text-xs text-neutral-400 leading-relaxed mb-4">{hint}</p>
        {requireText && (
          <input
            autoFocus
            className="input-dark w-full px-3 py-2 text-sm mb-4"
            placeholder={requireText}
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
        )}
        <div className="flex justify-end gap-2">
          <button
            className="u-btn u-btn-ghost px-3.5 py-1.5 text-xs"
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
            className="u-btn px-3.5 py-1.5 text-xs font-semibold disabled:opacity-40"
            style={{ background: "var(--u-danger-solid)", color: "#ffffff" }}
            disabled={!unlocked || busy}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

/* ---------- Panel（玻璃面板） ---------- */
export function Panel({
  strong = false,
  className,
  children,
}: {
  strong?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={cn(strong ? "glass-strong" : "glass", "rounded-xl", className)}
    >
      {children}
    </div>
  );
}

/* ---------- Chip（状态胶囊） ---------- */
export type ChipTone =
  "neutral" | "info" | "success" | "warn" | "danger" | "violet";

export function Chip({
  tone = "neutral",
  className,
  title,
  children,
}: {
  tone?: ChipTone;
  className?: string;
  title?: string;
  children: ReactNode;
}) {
  return (
    <span className={cn("u-chip", `u-chip-${tone}`, className)} title={title}>
      {children}
    </span>
  );
}

/* ---------- PageTitle ---------- */
export function PageTitle({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return <h2 className={cn("u-title text-lg", className)}>{children}</h2>;
}

/* ---------- EmptyState ---------- */
export function EmptyState({
  icon,
  children,
}: {
  icon: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="text-center">
      <div className="glass mx-auto mb-4 h-14 w-14 rounded-2xl grid place-items-center text-xl font-bold text-neutral-300">
        {icon}
      </div>
      <div className="text-sm text-neutral-500 whitespace-pre-line">
        {children}
      </div>
    </div>
  );
}

/* ---------- Loading / ErrorText ---------- */
export function Loading({ children }: { children: ReactNode }) {
  return <div className="p-8 text-sm text-neutral-500">{children}</div>;
}

export function ErrorText({ children }: { children: ReactNode }) {
  return <p className="text-sm text-rose-400">{children}</p>;
}

/* ---------- GithubMark（lucide 无品牌图标，官方 mark 内联） ---------- */
export function GithubMark({ size = 16 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden
    >
      <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
    </svg>
  );
}

/* ---------- SectionMark（分区字标：Docs/账户层等，逐字母入场，点击回应用） ---------- */
import { Link as RouterLink } from "@tanstack/react-router";
export function SectionMark({ text, title }: { text: string; title: string }) {
  return (
    <RouterLink
      to="/"
      title={title}
      className="relative inline-flex text-white text-[17px]"
      style={{ fontFamily: "var(--font-brand)", letterSpacing: "0.06em" }}
    >
      {[...text].map((ch, i) => (
        <span
          key={i}
          className="u-letter"
          style={{ animationDelay: `${80 + i * 45}ms` }}
        >
          {/* inline-flex 会折叠纯空格 span——换不折叠空格 */}
          {ch === " " ? " " : ch}
        </span>
      ))}
    </RouterLink>
  );
}

/* 时刻按看的人的时区显示。**只给时刻用**：recorded_at、created_at、上传时间这类
   "何时发生"的值。文档里写的日历日期（"2019 年 5 月"）没有时区，另走 ISO 切片，
   转成本地会让 UTC-5 的读者看到前一天。EntityHistory 里的判据同一条 */
export function localDate(iso: string): string {
  const d = new Date(iso);
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}
export function localDateTime(iso: string): string {
  const d = new Date(iso);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${localDate(iso)} ${hh}:${mm}`;
}
