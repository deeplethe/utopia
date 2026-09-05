/* Utopia UI 组件库 — 页面只用这里的组件与 styles.css 语义类，不写颜色字面量。
   规矩在 web/DESIGN.md，守卫在 scripts/style-guard.mjs：字号五档、间距六档、
   圆角两档、颜色只认令牌、状态（hover/focus/disabled/动效）只在这里定。
   Dialog / DangerConfirm / Tooltip / Table / Field 各在自己的文件里，从这里再导出。 */
import { forwardRef, useEffect, useRef, useState } from "react";
import type {
  ButtonHTMLAttributes,
  InputHTMLAttributes,
  ReactNode,
  SelectHTMLAttributes,
  TextareaHTMLAttributes,
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

/* ---------- Button ----------
   四种变体：primary（白底黑字，一屏最多一个）、secondary（描边，默认的次要动作）、
   ghost（无边框，行内动作与工具条）、danger（深红实底，不可逆的那一下）。
   两个尺寸与 Input 同高，同一行里顶齐不靠页面调 py。 */
export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: "sm" | "md";
  /** 文字左侧的图标（lucide，13 / 14 号） */
  icon?: ReactNode;
  /** 正在提交：禁用并告诉读屏器 */
  busy?: boolean;
};

const BUTTON_VARIANT: Record<ButtonVariant, string> = {
  primary: "u-btn-primary",
  secondary: "u-btn-secondary",
  ghost: "u-btn-quiet",
  danger: "u-btn-danger",
};

export function Button({
  variant = "secondary",
  size = "md",
  icon,
  busy,
  className,
  disabled,
  children,
  type = "button",
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cn(
        "u-btn",
        BUTTON_VARIANT[variant],
        size === "sm" ? "u-btn-sm" : "u-btn-md",
        className,
      )}
      disabled={disabled || busy}
      aria-busy={busy || undefined}
      {...props}
    >
      {icon && <span className="shrink-0">{icon}</span>}
      {children}
    </button>
  );
}

/* 只有图标的按钮：正方形；`label` 同时是 aria-label 与 title——
   没有可见文字的按钮必须有一个名字，这是无障碍的底线 */
export function IconButton({
  label,
  variant = "ghost",
  size = "md",
  className,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  label: string;
  variant?: ButtonVariant;
  size?: "sm" | "md";
}) {
  return (
    <button
      type={type}
      aria-label={label}
      title={label}
      className={cn(
        "u-btn",
        BUTTON_VARIANT[variant],
        size === "sm" ? "u-btn-icon-sm" : "u-btn-icon-md",
        className,
      )}
      {...props}
    />
  );
}

/* ---------- Input / Textarea / NativeSelect ---------- */
type InputSize = { size?: "sm" | "md" };

export function Input({
  className,
  size = "md",
  icon,
  ...props
}: Omit<InputHTMLAttributes<HTMLInputElement>, "size"> &
  InputSize & {
    /** 左侧的语义图标（筛选框的放大镜）。给了它，className 落在外层容器上 */
    icon?: ReactNode;
  }) {
  const control = (
    <input
      className={cn(
        "input-dark",
        size === "sm" ? "u-input-sm" : "u-input-md",
        icon ? (size === "sm" ? "pl-7" : "pl-8") : null,
        icon ? "w-full" : className,
      )}
      {...props}
    />
  );
  if (!icon) return control;
  return (
    <div className={cn("relative", className)}>
      <span
        className={cn(
          "pointer-events-none absolute top-1/2 -translate-y-1/2 text-ink-3",
          size === "sm" ? "left-2" : "left-3",
        )}
      >
        {icon}
      </span>
      {control}
    </div>
  );
}

export function Textarea({
  className,
  size = "md",
  ...props
}: TextareaHTMLAttributes<HTMLTextAreaElement> & InputSize) {
  return (
    <textarea
      className={cn(
        "input-dark u-scroll",
        size === "sm" ? "u-input-sm" : "u-input-md",
        className,
      )}
      {...props}
    />
  );
}

/* 原生 select：弹层无法主题化，所以只给"两三个选项、不值得一个 Dropdown"的地方用 */
export function NativeSelect({
  className,
  size = "md",
  ...props
}: Omit<SelectHTMLAttributes<HTMLSelectElement>, "size"> & InputSize) {
  return (
    <select
      className={cn(
        "input-dark appearance-none",
        size === "sm" ? "u-input-sm" : "u-input-md",
        className,
      )}
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
  const pad = size === "sm" ? "px-2.5 py-1 text-small" : "px-3 py-1.5 text-body";

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
        {icon && <span className="shrink-0 text-ink-3">{icon}</span>}
        <span className="flex-1 min-w-0 truncate">
          {current?.label ?? (
            <span className="text-ink-3">{placeholder ?? ""}</span>
          )}
        </span>
        <ChevronDown
          size={12}
          className={cn(
            "shrink-0 text-ink-3 transition-transform",
            open && "rotate-180",
          )}
        />
      </button>
      {open && (
        <div className="u-pop u-pop-in u-pop-in-tl absolute z-50 mt-1 w-full rounded-lg shadow-xl overflow-hidden">
          {menuLabel && (
            <div className="px-2.5 pt-2 pb-1 text-fine font-medium uppercase tracking-[0.1em] text-ink-3 border-b border-line">
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
                    ? "bg-surface-3 text-white"
                    : "text-ink-2 hover:bg-surface-2 hover:text-ink",
                )}
              >
                <span className="flex-1 min-w-0 truncate">{o.label}</span>
                {o.value === value && (
                  <Check size={12} className="shrink-0 text-ink-2" />
                )}
              </button>
            ))}
          </div>
          {footer && (
            <div
              className="border-t border-line"
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
    size === "sm" ? "pl-7 pr-2.5 py-1 text-small" : "pl-8 pr-3 py-1.5 text-body";
  const rowPad = size === "sm" ? "px-2.5 py-1 text-small" : "px-3 py-1.5 text-body";

  return (
    <div className={cn("relative", className)}>
      <SearchIcon
        size={size === "sm" ? 11 : 13}
        className="absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-3 pointer-events-none"
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
                  ? "bg-surface-3 text-white"
                  : "text-ink-2",
              )}
            >
              {!q && !!o.indent && (
                <span className="shrink-0" style={{ width: o.indent * 14 }} />
              )}
              <span className="min-w-0 flex-1 truncate">
                {o.label}
                {o.hint && (
                  <span className="ml-2 text-ink-3">{o.hint}</span>
                )}
              </span>
              {o.value === value && (
                <Check size={12} className="shrink-0 text-ink-2" />
              )}
            </button>
          ))}
          {visible.length === 0 && (
            <p className={cn(rowPad, "text-ink-3")}>{S.ui.noMatches}</p>
          )}
          {hidden > 0 && (
            <div
              className={cn(
                rowPad,
                "border-t border-line text-fine text-ink-3",
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
              className="group flex items-center gap-1 rounded-full bg-surface-2 px-2 py-0.5 text-fine text-ink transition-colors duration-fast hover:bg-surface-3"
              title={o.hint ?? o.label}
            >
              {o.label}
              <span className="text-ink-3 group-hover:text-ink">
                ✕
              </span>
            </button>
          ))}
        </div>
      )}
      {picked.length === 0 && emptyHint && (
        <p className="mb-1 text-fine text-ink-3">{emptyHint}</p>
      )}
      {/* 图标只对输入框定位。从前它相对整个组件居中，而组件里输入框上面
          还有一行已选项或空态提示，"一半高"就落到了输入框的上方（#288） */}
      <div className="relative">
        <SearchIcon
          size={11}
          className="absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-3 pointer-events-none"
        />
      <input
          ref={inputRef}
          className="input-dark w-full pl-7 pr-2.5 py-1 text-small"
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
      </div>
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
                "w-full flex items-center gap-2 text-left px-2.5 py-1 text-small",
                i === active
                  ? "bg-surface-3 text-white"
                  : "text-ink-2",
              )}
            >
              {!q && !!o.indent && (
                <span className="shrink-0" style={{ width: o.indent * 14 }} />
              )}
              <span className="min-w-0 flex-1 truncate">
                {o.label}
                {o.hint && (
                  <span className="ml-2 text-ink-3">{o.hint}</span>
                )}
              </span>
              {values.includes(o.value) && (
                <Check size={12} className="shrink-0 text-ink-2" />
              )}
            </button>
          ))}
          {visible.length === 0 && (
            <p className="px-2.5 py-1 text-small text-ink-3">
              {S.ui.noMatches}
            </p>
          )}
          {hidden > 0 && (
            <div className="px-2.5 py-1 text-fine text-ink-3 border-t border-line">
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
          className="h-8 w-14 rounded-lg border border-line-strong hover:border-line-strong transition-colors bg-surface grid place-items-center"
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
          className="h-8 w-14 rounded-lg border border-line-strong hover:border-line-strong transition-colors"
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
                    "outline outline-2 outline-ring outline-offset-1",
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
              "input-dark w-full px-2 py-1 text-small font-mono",
              !valid && "!border-danger",
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
    <div className={cn("flex items-center justify-end gap-2 text-small text-ink-3", className)}>
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
  return <h2 className={cn("u-title text-title", className)}>{children}</h2>;
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
      <div className="glass mx-auto mb-4 h-14 w-14 rounded-xl grid place-items-center text-title font-bold text-ink-2">
        {icon}
      </div>
      <div className="text-body text-ink-3 whitespace-pre-line">
        {children}
      </div>
    </div>
  );
}

/* ---------- Loading / ErrorText ---------- */
export function Loading({ children }: { children: ReactNode }) {
  return <div className="p-8 text-body text-ink-3">{children}</div>;
}

export function ErrorText({ children }: { children: ReactNode }) {
  return <p className="text-body text-danger">{children}</p>;
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
      className="relative inline-flex text-white text-title"
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

/* ---------- Row（可点的一行：左栏导航项、类树、关系/属性列表） ----------
   一行整条可点，指针停上变面、选中反白。列表里的行与左栏导航项是同一个东西，
   只差密度：nav 高一点、带图标；list 矮一点、可缩进。 */
/** 一行的类：Row 自己用；页面里必须是 <Link> 的行（跳去图谱的实例行）也用它 */
export function rowClass(active?: boolean, density: "nav" | "list" = "list"): string {
  return cn(
    "group flex w-full items-center gap-2 rounded-lg text-left transition-colors duration-fast",
    density === "nav" ? "px-2 py-2 text-body font-medium" : "px-2 py-1 text-body",
    active ? "u-nav-active" : "text-ink-2 hover:bg-surface-2 hover:text-ink",
  );
}
/** 行右端小字：静止时最淡，整行被指着时提亮一级 */
export const ROW_TRAILING = "ml-auto shrink-0 text-fine text-ink-3 group-hover:text-ink-2";

export function Row({
  active,
  density = "list",
  indent = 0,
  icon,
  trailing,
  className,
  children,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean;
  density?: "nav" | "list";
  /** 树形缩进的层级 */
  indent?: number;
  icon?: ReactNode;
  /** 右端的东西：计数、类型小字 */
  trailing?: ReactNode;
}) {
  return (
    <button
      type={type}
      aria-current={active ? "true" : undefined}
      style={indent ? { paddingLeft: `${8 + indent * 14}px` } : undefined}
      className={cn(rowClass(active, density), className)}
      {...props}
    >
      {icon && <span className="shrink-0 text-ink-3">{icon}</span>}
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {trailing && <span className={ROW_TRAILING}>{trailing}</span>}
    </button>
  );
}

/* 左栏底部那种钉住的入口：顶上一条线，不圆角，撑满 */
export function RailItem({
  active,
  icon,
  count,
  className,
  children,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean;
  icon?: ReactNode;
  count?: number;
}) {
  return (
    <button
      type={type}
      aria-current={active ? "true" : undefined}
      className={cn(
        "flex w-full shrink-0 items-center gap-2 border-t border-line px-4 py-2 text-left text-body transition-colors duration-fast",
        active ? "u-nav-active" : "text-ink-2 hover:bg-surface-2 hover:text-ink",
        className,
      )}
      {...props}
    >
      {icon && <span className="shrink-0 text-ink-3">{icon}</span>}
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {count !== undefined && count > 0 && (
        <span className="u-num ml-auto rounded-full bg-surface-3 px-2 text-fine text-ink-3">
          {count}
        </span>
      )}
    </button>
  );
}

/* ---------- Segmented（分段切换） ----------
   两三个互斥的视图或取值。fill = 每格等宽撑满（左栏的类/属性切换）；
   否则按内容宽（连接方向、形状那种小开关）。 */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  size = "md",
  fill,
  className,
}: {
  value: T;
  options: { value: T; label: ReactNode; count?: number; title?: string }[];
  onChange: (v: T) => void;
  size?: "sm" | "md";
  fill?: boolean;
  className?: string;
}) {
  return (
    <div
      role="tablist"
      className={cn(
        "flex gap-1 rounded-lg bg-surface p-1",
        fill && "w-full",
        className,
      )}
    >
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="tab"
            aria-selected={active}
            title={o.title}
            onClick={() => onChange(o.value)}
            className={cn(
              "flex items-center justify-center gap-1 rounded-lg font-medium transition-colors duration-fast",
              size === "sm" ? "px-2 py-1 text-fine" : "px-3 py-1 text-small",
              fill && "flex-1",
              active
                ? "bg-surface-3 text-ink"
                : "text-ink-3 hover:bg-surface-2 hover:text-ink-2",
            )}
          >
            {o.label}
            {o.count !== undefined && o.count > 0 && (
              <span className="u-num text-ink-3">{o.count}</span>
            )}
          </button>
        );
      })}
    </div>
  );
}

/* ---------- Checkbox ---------- */
export function Checkbox({
  label,
  hint,
  className,
  ...props
}: Omit<InputHTMLAttributes<HTMLInputElement>, "type"> & {
  label: ReactNode;
  hint?: ReactNode;
}) {
  return (
    <label className={cn("flex cursor-pointer items-start gap-2", className)}>
      <input type="checkbox" className="mt-1 accent-accent" {...props} />
      <span className="min-w-0">
        <span className="block text-body text-ink">{label}</span>
        {hint && <span className="block text-fine text-ink-3">{hint}</span>}
      </span>
    </label>
  );
}

/* ---------- Disclosure（折叠小节：一行可点的摘要 + 展开的内容） ---------- */
export function Disclosure({
  summary,
  defaultOpen,
  className,
  children,
}: {
  summary: ReactNode;
  defaultOpen?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <details open={defaultOpen} className={className}>
      <summary className="cursor-pointer select-none text-small text-ink-3 transition-colors duration-fast hover:text-ink-2">
        {summary}
      </summary>
      <div className="mt-2">{children}</div>
    </details>
  );
}

/* ---------- ToolTower（画布上的竖排工具塔） ----------
   图标常驻，名字在整组 hover / 键盘走到时一起展开（styles.css 的 u-tower）。
   一组是一个语义单元：派生 / 布局 / 相机。 */
export function ToolTower({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        "u-tower group glass-strong flex flex-col overflow-hidden rounded-xl shadow-xl",
        className,
      )}
    >
      {children}
    </div>
  );
}

export const ToolButton = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & {
    active?: boolean;
    /** 展开时显示的名字，也是 title 与无障碍名称 */
    label: string;
    icon: ReactNode;
  }
>(function ToolButton({ active, label, icon, className, type = "button", ...props }, ref) {
  return (
    <button
      ref={ref}
      type={type}
      title={label}
      aria-label={label}
      className={cn("u-tool", active && "is-on", className)}
      {...props}
    >
      {icon}
      <span className="u-tower-label">{label}</span>
    </button>
  );
});

export function ToolDivider() {
  return <div className="mx-2 h-px bg-line-strong" />;
}

/* ---------- Pill（玻璃药丸：图例、"+N 个类"这类浮在画布上的小开关） ---------- */
export const Pill = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & {
    active?: boolean;
    /** 被关掉的那种：压到三成五 */
    dim?: boolean;
  }
>(function Pill({ active, dim, className, type = "button", ...props }, ref) {
  return (
    <button
      ref={ref}
      type={type}
      className={cn("u-pill", active && "is-on", dim && "is-dim", className)}
      {...props}
    />
  );
});

/* ---------- Radio ---------- */
export function Radio({
  label,
  className,
  children,
  ...props
}: Omit<InputHTMLAttributes<HTMLInputElement>, "type"> & {
  label: ReactNode;
  /** 选中后跟在标签后面的东西（比如一个日期框） */
  children?: ReactNode;
}) {
  return (
    <label className={cn("flex items-center gap-2 text-small text-ink-2", className)}>
      <input type="radio" className="accent-accent" {...props} />
      {label}
      {children}
    </label>
  );
}

/* ---------- ExpandCard（可展开的一条：事实、派生、被挡下的派生） ----------
   头是一整条可点的按钮，展开的内容跟在下面。头里可以有 role="link" 的 span
   （去看另一端），但不能有按钮——按钮里不能嵌按钮。 */
export function ExpandCard({
  open,
  onToggle,
  dim,
  title,
  headerClassName,
  className,
  header,
  children,
}: {
  open: boolean;
  onToggle: () => void;
  /** 陈旧的那种：整条压淡 */
  dim?: boolean;
  title?: string;
  headerClassName?: string;
  className?: string;
  header: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div
      className={cn("u-card-row group", open && "is-open", dim && "opacity-55", className)}
      title={title}
    >
      <button
        type="button"
        onClick={onToggle}
        className={cn("w-full px-2 py-1 text-left", headerClassName)}
      >
        {header}
      </button>
      {children}
    </div>
  );
}

/** 复合行的外壳：一行里有两个按钮时不能是 Row（按钮里不能嵌按钮），
    外层 div 用它拿到 hover 与 group */
export const HOVER_ROW =
  "group flex items-center gap-2 rounded-lg px-2 py-1 transition-colors duration-fast hover:bg-surface-2";
/** 指针停在所在行（.group）上才现身的东西；加 is-on 常显 */
export const REVEAL = "u-reveal";

/* 各在自己文件里的组件，从这里一并导出，页面只认 "../ui" 一个入口 */
export { Dialog, DangerConfirm } from "./dialog";
export { Tooltip } from "./tooltip";
export { Table, THead, TBody, Tr, Th, Td } from "./table";
export { Field } from "./field";
