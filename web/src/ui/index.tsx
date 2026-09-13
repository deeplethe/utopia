/* Utopia UI 组件库 — 页面只用这里的组件与 styles.css 语义类，不写颜色字面量。
   规矩在 web/DESIGN.md，守卫在 scripts/style-guard.mjs：字号五档、间距六档、
   圆角四档、颜色只认令牌、状态（hover/focus/disabled/动效）只在这里定。
   Dialog / DangerConfirm / Tooltip / Table / Field 各在自己的文件里，从这里再导出。 */
import { forwardRef, useEffect, useRef, useState,
  useId,} from "react";
import type {
  ButtonHTMLAttributes,
  CSSProperties,
  InputHTMLAttributes,
  ReactNode,
  TextareaHTMLAttributes,
} from "react";
import {
  ArrowUpRight,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  Loader2,
  Search as SearchIcon,
} from "lucide-react";
import { S } from "../i18n";
import { Button as ShadButton, buttonVariants } from "@/components/ui/button";
import { badgeVariants } from "@/components/ui/badge";
import { Checkbox as ShadCheckbox } from "@/components/ui/checkbox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import {
  DropdownMenu as ShadDropdownMenu,
  DropdownMenuContent as ShadDropdownMenuContent,
  DropdownMenuLabel as ShadDropdownMenuLabel,
  DropdownMenuRadioGroup as ShadDropdownMenuRadioGroup,
  DropdownMenuRadioItem as ShadDropdownMenuRadioItem,
  DropdownMenuSeparator as ShadDropdownMenuSeparator,
  DropdownMenuTrigger as ShadDropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover as ShadPopover,
  PopoverContent as ShadPopoverContent,
  PopoverTrigger as ShadPopoverTrigger,
} from "@/components/ui/popover";
import {
  Command as ShadCommand,
  CommandEmpty as ShadCommandEmpty,
  CommandGroup as ShadCommandGroup,
  CommandInput as ShadCommandInput,
  CommandItem as ShadCommandItem,
  CommandList as ShadCommandList,
} from "@/components/ui/command";
import {
  RadioGroup as ShadRadioGroup,
  RadioGroupItem as ShadRadioGroupItem,
} from "@/components/ui/radio-group";
import { Input as ShadInput } from "@/components/ui/input";
import { Textarea as ShadTextarea } from "@/components/ui/textarea";
// 表格原件在 ./table 里，下面 re-export；`SkeletonTableRows` 自己也要用，
// 所以这里另取一份别名，避免与 re-export 的同名标识撞车
import {
  Table as SkelTable,
  TBody as SkelTBody,
  Td as SkelTd,
  Tr as SkelTr,
} from "./table";

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
      className={cn("relative inline-flex text-ink", className)}
      style={{ fontFamily: "var(--font-brand)", letterSpacing: "0.01em" }}
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
   **壳在这里，样子在 shadcn**（`@/components/ui/button`，radix base / nova preset）。
   这一层留着的理由只有一个：整个应用按这四个名字调按钮——primary 是一屏最多一个的
   实心，secondary 是描边的默认次要动作，ghost 是行内与工具条，danger 是不可逆的那一下。
   名字说的是**这一下有多重**，shadcn 那边说的是长什么样，两件事分开，
   换皮肤不必回头改三十二个页面。 */
export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
type ButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "color"> & {
  variant?: ButtonVariant;
  size?: "sm" | "md";
  /** 文字左侧的图标（lucide）。shadcn 那边已经给 svg 定了尺寸与 shrink-0 */
  icon?: ReactNode;
  /** 正在提交：禁用并告诉读屏器 */
  busy?: boolean;
};

const VARIANT = {
  primary: "default",
  // 我们的 secondary 是**描边**的，shadcn 的 secondary 是实心灰，对应的是 outline
  secondary: "outline",
  ghost: "ghost",
  danger: "destructive",
} as const;

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  {
    variant = "secondary",
    size = "md",
    icon,
    busy,
    className,
    disabled,
    children,
    type = "button",
    ...props
  },
  ref,
) {
  return (
    <ShadButton
      ref={ref}
      type={type}
      variant={VARIANT[variant]}
      size={size === "sm" ? "sm" : "default"}
      className={className}
      disabled={disabled || busy}
      aria-busy={busy || undefined}
      {...props}
    >
      {icon}
      {children}
    </ShadButton>
  );
});

/* 只有图标的按钮：正方形；`label` 同时是 aria-label 与 title——
   没有可见文字的按钮必须有一个名字，这是无障碍的底线 */
export const IconButton = forwardRef<
  HTMLButtonElement,
  Omit<ButtonHTMLAttributes<HTMLButtonElement>, "color"> & {
    label: string;
    variant?: ButtonVariant;
    size?: "sm" | "md";
  }
>(function IconButton(
  { label, variant = "ghost", size = "md", className, type = "button", ...props },
  ref,
) {
  return (
    <ShadButton
      ref={ref}
      type={type}
      aria-label={label}
      title={label}
      variant={VARIANT[variant]}
      size={size === "sm" ? "icon-sm" : "icon"}
      className={className}
      {...props}
    />
  );
});

/** 长得像按钮的**链接**（路由 Link、外链）。给 className 用，不包组件——
 *  `<Link className={buttonLike("ghost")}>`；从前这里写的是 `u-btn u-btn-ghost`
 *  加一串手调的内距，那串内距正是尺寸档存在的理由 */
export function buttonLike(
  variant: ButtonVariant = "ghost",
  size: "sm" | "md" = "md",
): string {
  return buttonVariants({
    variant: VARIANT[variant],
    size: size === "sm" ? "sm" : "default",
  });
}

/* ---------- Input / Textarea ----------
   同 Button：壳在这里，皮在 shadcn。这一层留的是两件 shadcn 不管的事——
   **图标槽**（左栏那道带放大镜的筛选框，图标要与下面每一行的图标落在同一条
   竖线上）和 **bare**（装在别的面里的输入，自己不带皮：切换器顶上的查找、
   对话的输入区）。尺寸仍按我们的两档说话，映到 shadcn 的高度上。 */
type InputSize = { size?: "sm" | "md" };

/** 没有自己的皮的那一档：去边框、去底、去焦点环，交给外面那块面 */
const BARE =
  "border-0 bg-transparent shadow-none focus-visible:ring-0 focus-visible:border-0 px-0 dark:bg-transparent";

/** 一个"框"的皮：边、圆角、实底、焦点环，与 `Input` 完全同一副。
 *  下拉、搜索选择器这些**自己画触发器**的控件用它——从前它们蹭的是手写的
 *  `.input-dark`，那个类随按钮/输入框迁移删掉了，于是触发器一夜之间没了皮
 *  （"怎么有没背景的框"）。皮只此一份，谁要谁引。 */
export const FIELD_SHELL =
  "flex items-center rounded-control border border-input bg-background text-body text-ink transition-colors outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-50";

/** 有皮的那一档**要有实底**。shadcn 默认 `bg-transparent`：在一张卡片里那是对的，
 *  可这套界面的输入框有浮在图谱画布上的（搜索实体、筛选），透明的框底下是网格和
 *  连线，字压在线上，看着就是一团乱。取页面底色——在卡片上它比卡片深一点，
 *  在画布上它是一块实地，两处都读得出"这是一个可以写字的格子"。 */
const FILLED = "bg-background dark:bg-background";

export const Input = forwardRef<
  HTMLInputElement,
  Omit<InputHTMLAttributes<HTMLInputElement>, "size"> &
    InputSize & {
      /** 左侧的语义图标（筛选框的放大镜）。给了它，className 落在外层容器上 */
      icon?: ReactNode;
      /** 没有自己的皮：装在别的面里（切换器面板顶上那道查找） */
      bare?: boolean;
    }
>(function Input({ className, size = "md", icon, bare, ...props }, ref) {
  const control = (
    <ShadInput
      ref={ref}
      className={cn(
        /* bare 的输入框**连控件的高度也不要**。`bare` 说的是「它是面板的一段，
           不是面板里摆的一个控件」，可它还顶着 h-8 + py-1：32 高的盒子里装一行
           20 的字，剩下那 12px 不是对半分的，字就往下坐了一点，看着是上面的留白
           比下面大。高度交还给行高，竖向留白交还给外面那块面（告警面板、库切换器
           都是 py-3），上下就真的一样了。
           textarea 那一档不动：它的高度本来就是 rows 给的，不是控件档位。 */
        bare ? cn(BARE, "h-auto py-0") : cn(FILLED, size === "sm" ? "h-7" : "h-8"),
        // 图标槽：中号图标离左内缘 12px、文字从 34px 起；小号窄一档（8 / 28）
        icon ? (size === "sm" ? "pl-7" : "pl-[34px]") : null,
        icon ? "w-full" : className,
      )}
      {...props}
    />
  );
  if (!icon) return control;
  return (
    /* **外层要跟控件同一个圆角**：带图标时 `className` 落在这一层（宽度、
       投影都写在调用处），而投影是按这个盒子的形状画的——盒子没有圆角，
       投影就是方的，里面那个圆角的输入框浮在一块方影子上，四个角看着发虚 */
    <div className={cn("relative rounded-control", className)}>
      {/* 图标离盒左缘 12——与 nav 行的内距同一个数，于是左栏里输入框的放大镜
          与下面每一行的图标落在同一条竖线上（盒 8 / 图标 20 / 文字 42） */}
      <span
        className={cn(
          "pointer-events-none absolute top-1/2 -translate-y-1/2 text-ink-2",
          size === "sm" ? "left-2" : "left-3",
        )}
      >
        {icon}
      </span>
      {control}
    </div>
  );
});

export const Textarea = forwardRef<
  HTMLTextAreaElement,
  TextareaHTMLAttributes<HTMLTextAreaElement> &
    InputSize & {
      /** 没有自己的皮：装在别的面里（对话输入框那种） */
      bare?: boolean;
    }
>(function Textarea({ className, size = "md", bare, ...props }, ref) {
  return (
    <ShadTextarea
      ref={ref}
      className={cn(
        "u-scroll",
        bare ? BARE : cn(FILLED, size === "sm" ? "min-h-14" : null),
        className,
      )}
      {...props}
    />
  );
});

/* 原生 select：弹层无法主题化，所以只给"两三个选项、不值得一个 Dropdown"的地方用 */
/* `NativeSelect` 去了：页面里一个都不剩。原生 select 的弹层是操作系统画的，
   主题化不了——同一页上两种下拉，一种是我们的面，一种是系统的灰框。留着一个
   没人用的组件，只会让它某天又溜回来。小而有界的枚举用 `Dropdown`，
   成百上千的（本体的类、部署里的人）用 `SearchSelect`。 */

/* ---------- Dropdown（小而有界的枚举） ----------
   **壳在这里，弹层是 shadcn 的 DropdownMenu**。从前这里连同 SearchSelect 各自
   实现过一遍「点外面关掉、Esc 关掉、方向键选、焦点回到触发器」，三处弹层三套
   行为，还各自缺一点：手搓的入口连 `aria-haspopup` 都没有。
   接口一个字没改（value / options / onChange / icon / menuLabel / footer），
   页面照旧。 */
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
  const current = options.find((o) => o.value === value);
  return (
    <ShadDropdownMenu>
      <ShadDropdownMenuTrigger asChild>
        <button
          type="button"
          title={menuLabel}
          className={cn(
            FIELD_SHELL,
            /* **调用方给了宽度就不塞 `w-full`**：这里的 `cn` 只是把类拼起来，
               不做 Tailwind 的冲突消解，两个 `w-` 同时在场时谁赢看生成顺序，
               不看写的顺序——`className="w-40"` 曾经完全不起作用 */
            className?.includes("w-") ? null : "w-full",
            "justify-between gap-2 text-left",
            size === "sm" ? "h-7 px-2.5 text-small" : "h-8 px-3 text-body",
            className,
          )}
        >
          <span className="flex min-w-0 items-center gap-2">
            {icon && <span className="shrink-0 text-ink-2">{icon}</span>}
            <span className={cn("truncate", !current && "text-ink-2")}>
              {current?.label ?? placeholder ?? ""}
            </span>
          </span>
          <ChevronDown size={12} className="shrink-0 text-ink-2" />
        </button>
      </ShadDropdownMenuTrigger>
      <ShadDropdownMenuContent align="start" className="min-w-(--radix-dropdown-menu-trigger-width)">
        {menuLabel && <ShadDropdownMenuLabel>{menuLabel}</ShadDropdownMenuLabel>}
        <ShadDropdownMenuRadioGroup value={value} onValueChange={onChange}>
          {options.map((o) => (
            <ShadDropdownMenuRadioItem key={o.value} value={o.value}>
              {o.label}
            </ShadDropdownMenuRadioItem>
          ))}
        </ShadDropdownMenuRadioGroup>
        {footer && (
          <>
            <ShadDropdownMenuSeparator />
            {footer}
          </>
        )}
      </ShadDropdownMenuContent>
    </ShadDropdownMenu>
  );
}

/* ---------- SearchSelect（可搜索选择器：无界对象列表专用——成员、父类、数据源…）
   触发器同 Dropdown，弹层是 shadcn 的 Popover + Command：搜、键盘上下、
   回车选中、空结果的那一行，全由 Command 管。 */
export interface SearchSelectOption {
  value: string;
  label: string;
  /** 跟在名字后面的一小行（邮箱、类型、路径），也参与搜索 */
  hint?: string;
  /** 树形缩进的层级（父类选择器用它） */
  indent?: number;
}

export function SearchSelect({
  value,
  options,
  onChange,
  placeholder,
  className,
  size = "md",
}: {
  value: string;
  options: SearchSelectOption[];
  onChange: (v: string) => void;
  placeholder?: string;
  className?: string;
  size?: "sm" | "md";
  /** 只影响弹层高度，保留是为了调用处不改 */
  maxVisible?: number;
}) {
  const [open, setOpen] = useState(false);
  const current = options.find((o) => o.value === value);
  return (
    <ShadPopover open={open} onOpenChange={setOpen}>
      <ShadPopoverTrigger asChild>
        <button
          type="button"
          role="combobox"
          aria-expanded={open}
          className={cn(
            FIELD_SHELL,
            /* **调用方给了宽度就不塞 `w-full`**：这里的 `cn` 只是把类拼起来，
               不做 Tailwind 的冲突消解，两个 `w-` 同时在场时谁赢看生成顺序，
               不看写的顺序——`className="w-40"` 曾经完全不起作用 */
            className?.includes("w-") ? null : "w-full",
            "justify-between gap-2 text-left",
            size === "sm" ? "h-7 px-2.5 text-small" : "h-8 px-3 text-body",
            className,
          )}
        >
          <span className={cn("truncate", !current && "text-ink-2")}>
            {current?.label ?? placeholder ?? ""}
          </span>
          <ChevronDown size={12} className="shrink-0 text-ink-2" />
        </button>
      </ShadPopoverTrigger>
      <ShadPopoverContent align="start" className="w-(--radix-popover-trigger-width) p-0">
        <ShadCommand>
          <ShadCommandInput placeholder={placeholder ?? ""} />
          <ShadCommandList>
            <ShadCommandEmpty>{S.ui.noMatches}</ShadCommandEmpty>
            <ShadCommandGroup>
              {options.map((o) => (
                <ShadCommandItem
                  key={o.value}
                  value={`${o.label} ${o.hint ?? ""}`}
                  onSelect={() => {
                    onChange(o.value);
                    setOpen(false);
                  }}
                >
                  <span className="truncate">{o.label}</span>
                  {o.hint && (
                    <span className="ml-auto truncate text-fine text-ink-2">{o.hint}</span>
                  )}
                </ShadCommandItem>
              ))}
            </ShadCommandGroup>
          </ShadCommandList>
        </ShadCommand>
      </ShadPopoverContent>
    </ShadPopover>
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
              className="group flex items-center gap-1 rounded-cell bg-surface-2 px-2 py-0.5 text-fine text-ink transition-colors duration-fast hover:bg-surface-3"
              title={o.hint ?? o.label}
            >
              {o.label}
              <span className="text-ink-2 group-hover:text-ink">
                ✕
              </span>
            </button>
          ))}
        </div>
      )}
      {picked.length === 0 && emptyHint && (
        <p className="mb-1 text-fine text-ink-2">{emptyHint}</p>
      )}
      {/* 图标只对输入框定位。从前它相对整个组件居中，而组件里输入框上面
          还有一行已选项或空态提示，"一半高"就落到了输入框的上方（#288） */}
      <div className="relative">
        <SearchIcon
          size={11}
          className="absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-2 pointer-events-none"
        />
      <input
          ref={inputRef}
          className={cn(FIELD_SHELL, "w-full pl-7 pr-2.5 py-1 text-small")}
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
        <div className="u-menu-glass u-pop-in u-pop-in-tl absolute z-50 mt-1 w-full rounded-overlay u-lift-strong overflow-hidden">
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
                  ? "bg-surface-3 text-ink"
                  : "text-ink-2",
              )}
            >
              {!q && !!o.indent && (
                <span className="shrink-0" style={{ width: o.indent * 14 }} />
              )}
              <span className="min-w-0 flex-1 truncate">
                {o.label}
                {o.hint && (
                  <span className="ml-2 text-ink-2">{o.hint}</span>
                )}
              </span>
              {values.includes(o.value) && (
                <Check size={12} className="shrink-0 text-ink-2" />
              )}
            </button>
          ))}
          {visible.length === 0 && (
            <p className="px-2.5 py-1 text-small text-ink-2">
              {S.ui.noMatches}
            </p>
          )}
          {hidden > 0 && (
            <div className="px-2.5 py-1 text-fine text-ink-2 border-t border-line">
              {S.ui.keepTyping(hidden)}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

import { ENTITY_PALETTE } from "../palette";
export { ENTITY_PALETTE, colorForKey } from "../palette";

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
          className="h-8 w-10 rounded-control border border-line-strong hover:border-line-strong transition-colors bg-surface grid place-items-center"
        >
          <span
            className={cn("h-3.5 w-3.5", shape === "circle" ? "rounded-full" : "scale-90")}
            style={{ background: valid ? value : ENTITY_PALETTE[0] }}
          />
        </button>
      ) : (
        <button
          type="button"
          title={value}
          onClick={() => setOpen(!open)}
          className="h-8 w-14 rounded-control border border-line-strong hover:border-line-strong transition-colors"
          style={{ background: valid ? value : ENTITY_PALETTE[0] }}
        />
      )}
      {open && (
        // 显式宽度：绝对定位的收缩宽度会被 inline-block 触发器的容器块钳死
        <div className="u-menu-glass u-pop-in u-pop-in-tl absolute z-50 left-0 top-full mt-2 w-56 rounded-overlay p-3 u-lift-strong">
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
              FIELD_SHELL,
              "w-full px-2 py-1 text-small font-mono",
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
  always,
  /** 覆盖默认的上边距。默认 `mt-3` 适合跟在列表后面；
      放进一个已经有内边距的底栏时传 `""` 去掉它 */
  className = "mt-3",
}: {
  total: number;
  pageSize: number;
  page: number;
  onPage: (p: number) => void;
  /** 只有一页也照样显示。给的是**固定底栏**用：那条栏本来就在那儿，
      分页器一藏，它就成了一道没有内容的空边 */
  always?: boolean;
  className?: string;
}) {
  const pageCount = Math.max(1, Math.ceil(total / pageSize));
  const safe = Math.min(page, pageCount - 1);
  if (total <= pageSize && !always) return null;
  return (
    <div className={cn("flex items-center justify-end gap-2 text-small text-ink-2", className)}>
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
        className="u-btn u-btn-ghost h-7 w-7 grid place-items-center"
      >
        <ChevronLeft size={13} />
      </button>
      <button
        onClick={() => onPage(safe + 1)}
        disabled={safe >= pageCount - 1}
        className="u-btn u-btn-ghost h-7 w-7 grid place-items-center"
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
      className={cn(strong ? "glass-strong" : "glass", "rounded-panel", className)}
    >
      {children}
    </div>
  );
}

/* ---------- SettingsCard（一个保存单位） ----------
   设置页里的一张卡：标题、一句说明、字段，底下一条横栏——左边是约束或代价，
   右边是这张卡自己的保存。**边框圈的是这个按钮管到哪儿**：改了名字点保存，
   不该把下面四个开关一起送上去（DESIGN.md 6）。
   保存用 secondary：一屏最多一个 primary（规矩 5），而设置页上每张卡都有一个。 */
export function SettingsCard({
  title,
  /** 标题下的一句：这个设置是什么 */
  hint,
  /** 底栏左边的一句：约束、代价、什么时候生效 */
  note,
  /** 底栏右边：通常是这张卡的保存按钮 */
  action,
  className,
  children,
}: {
  title: ReactNode;
  hint?: ReactNode;
  note?: ReactNode;
  action?: ReactNode;
  className?: string;
  children?: ReactNode;
}) {
  return (
    <section className={cn("glass overflow-hidden rounded-panel", className)}>
      <div className="p-6">
        <h2 className="text-title text-ink">{title}</h2>
        {hint && <p className="mt-1 text-small leading-relaxed text-ink-2">{hint}</p>}
        {children && <div className="mt-4">{children}</div>}
      </div>
      {(note || action) && (
        /* 底栏只靠一条线与上面分开，不另铺一层面：surface 的三档是静止/悬停/选中，
           拿悬停那档当静止的底会让这条栏看起来一直被指着 */
        <div className="flex items-center justify-between gap-4 border-t border-line px-6 py-3">
          <div className="min-w-0 flex-1 text-small leading-relaxed text-ink-2">{note}</div>
          <div className="flex shrink-0 items-center gap-3">{action}</div>
        </div>
      )}
    </section>
  );
}

/* ---------- StatusCell（表里的一格状态） ----------
   **顺利就是普通文字，出事才有颜色。** 一列里每行都挂着一个彩色胶囊时，
   颜色不再指示任何东西——十二个「Ready」和一个「Failed」长得一样重，
   眼睛得逐行读才找得到坏的那一行。所以状态词就是次要色的一句话，
   失败在前面加一个红色的叹号：**红的是那个符号，不是整句话**，
   一列扫下来只有几个红点跳出来。
   给了 onClick 就是可点的（点开看报错原文）。 */
export function StatusCell({
  danger,
  onClick,
  title,
  children,
}: {
  danger?: boolean;
  onClick?: () => void;
  title?: string;
  children: ReactNode;
}) {
  const body = (
    <>
      {danger && <CircleAlert size={12} className="shrink-0 text-danger" />}
      {children}
    </>
  );
  const cls = "inline-flex items-center gap-1.5 text-small text-ink-2";
  if (!onClick) {
    return (
      <span className={cls} title={title}>
        {body}
      </span>
    );
  }
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={cn(cls, "u-linkbtn")}
    >
      {body}
    </button>
  );
}

/* ---------- Status（一个状态） ----------
   **颜色只上那个点，字不上色。**从前状态是一枚填色的胶囊（绿底绿字的
   "Confirmed"、琥珀底琥珀字的 "Pending"），一列表格里十几个填色块，
   眼睛先看见的是一片颜色，而不是那一列在说什么。点只占它该占的那一点地方，
   颜色仍然分得出成没成、等不等人，字回到正文的灰。

   `Chip` 留给**不是状态**的东西：计数、库名、公理名、"派生"这类标记——
   它们是贴在内容上的标签，本来就该有个盒子把自己圈出来。 */
export function Status({
  tone = "neutral",
  pulse,
  className,
  title,
  children,
}: {
  tone?: ChipTone;
  /** 这件事**正在进行**：点跟着呼吸。给「agent 正在裁这一对」这种活状态用 */
  pulse?: boolean;
  className?: string;
  title?: string;
  children: ReactNode;
}) {
  return (
    <span
      className={cn("inline-flex items-center gap-1.5 text-small text-ink-2", className)}
      title={title}
    >
      <span
        className={cn(
          "h-1.5 w-1.5 shrink-0 rounded-full",
          STATUS_DOT[tone],
          pulse && "animate-pulse",
        )}
      />
      {children}
    </span>
  );
}

const STATUS_DOT: Record<ChipTone, string> = {
  neutral: "bg-ink-2",
  success: "bg-ok",
  warn: "bg-warn",
  danger: "bg-danger",
  info: "bg-violet",
  violet: "bg-violet",
  contest: "bg-contest",
};

/* ---------- Chip（状态胶囊） ---------- */
export type ChipTone =
  "neutral" | "info" | "success" | "warn" | "danger" | "violet" | "contest";

export function Chip({
  tone = "neutral",
  className,
  title,
  onClick,
  children,
}: {
  tone?: ChipTone;
  className?: string;
  title?: string;
  /** 给了就是一个按钮（点开看失败原文那种） */
  onClick?: () => void;
  children: ReactNode;
}) {
  /* 形状走 shadcn 的 Badge，**颜色仍是我们的语义色**：Badge 只有
     default / secondary / destructive 这几档，而 chip 在这套语汇里说的是
     状态——ready、3 dropped、contested、derived 各有各的色，它们是令牌，
     不是变体。所以底用 outline 的骨架，色按 tone 贴上去。 */
  const cls = cn(
    badgeVariants({ variant: "outline" }),
    /* **方角，不是药丸**：shadcn 的 badge 底子是 `rounded-4xl`，一位数的计数
       会缩成一个圆点。这套语汇里 chip 说的是状态（Ready、3 dropped、
       contested），是一块牌子不是一颗豆子，走 cell 那一档（规矩 3）。
       几何（高 20、内距 8、字号 xs）仍照 shadcn 的来 */
    "rounded-cell border-transparent",
    CHIP_TONE[tone],
    onClick && "cursor-pointer",
    className,
  );
  if (onClick) {
    return (
      <button type="button" onClick={onClick} title={title} className={cls}>
        {children}
      </button>
    );
  }
  return (
    <span className={cls} title={title}>
      {children}
    </span>
  );
}

/** chip 的样子，给不能是组件的地方用：行内那个 `role="link"` 的 span
 *  （外面已经是一条可点的行，按钮里不能再套按钮） */
export function chipLike(tone: ChipTone = "neutral", className?: string): string {
  return cn(
    badgeVariants({ variant: "outline" }),
    "rounded-cell border-transparent",
    CHIP_TONE[tone],
    className,
  );
}

const CHIP_TONE: Record<ChipTone, string> = {
  neutral: "bg-surface-2 text-ink-2",
  success: "bg-ok/12 text-ok",
  warn: "bg-warn/12 text-warn",
  danger: "bg-danger/12 text-danger",
  info: "bg-violet/12 text-violet",
  contest: "bg-contest/12 text-contest",
  violet: "bg-violet/12 text-violet",
};

/* ---------- LinkButton（长得像一句话的动作：表格行末的"重抽""删除"） ---------- */
export function LinkButton({
  tone = "default",
  underline,
  className,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  tone?: "default" | "danger";
  /** 带下划线（u-link 那种） */
  underline?: boolean;
}) {
  return (
    <button
      type={type}
      className={cn(
        "u-linkbtn",
        tone === "danger" && "is-danger",
        underline && "u-link",
        className,
      )}
      {...props}
    />
  );
}

/* ---------- ChoiceCard（勾选框藏起来的可选卡片：本体包那种并排的几张） ---------- */
export function ChoiceCard({
  checked,
  onChange,
  label,
  className,
  children,
}: {
  checked: boolean;
  onChange: () => void;
  /** 读屏器念的名字 */
  label: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <label className={cn("u-choice", checked && "is-on", className)}>
      <input
        type="checkbox"
        className="sr-only"
        aria-label={label}
        checked={checked}
        onChange={onChange}
      />
      {children}
    </label>
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

/** 一张卡片底下那排动作的形状。**只此一份**：审阅页有七种卡（重复对、待确认的
 *  事实、时态冲突、过期断言、本体缺陷、公理冲突、人提的事实），从前它们三种站法
 *  都有——三张靠右下、三张靠左下、两张干脆挂在某一行的右端。一个人从一张卡走到
 *  下一张，每次都要重新找「裁决在哪」。
 *
 *  规矩两条：**动作在卡片左下**，与卡片里每一行文字同一条左边线，一列卡下来
 *  按钮落在同一条竖线上，指针几乎不用横向移动；**会换行**，因为有的卡有五六个
 *  选项，靠右排一旦折行，左缘就参差不齐。
 *
 *  还有一条不在这个类里、但同样是规矩：**危险的那一个永远排在最后**。左对齐之后
 *  最左边是指针最先够到的位置，那里不该是「拒绝」。 */
export const CARD_ACTIONS = "mt-3 flex flex-wrap items-center gap-2";

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
      <div className="glass mx-auto mb-4 h-14 w-14 rounded-panel grid place-items-center text-title font-bold text-ink-2">
        {icon}
      </div>
      <div className="text-body text-ink-2 whitespace-pre-line">
        {children}
      </div>
    </div>
  );
}

/* ---------- Loading / Skeleton / Spinner / ErrorText ---------- */
export function Loading({ children }: { children: ReactNode }) {
  return <div className="p-8 text-body text-ink-2">{children}</div>;
}

/** 一条骨架。**形状已知的东西用它**：一行标题、一枚色点、一列树。
 *
 * 尺寸由调用方给（高度用字号那几档的高度，宽度用 `style`——条的长短是照
 * 真内容的长短分布随手定的，一列等宽看着像进度条不像清单）。圆角固定 cell：
 * 它顶替的是一格内容，不是一块面。 */
export function Skeleton({
  className,
  style,
}: {
  className?: string;
  style?: CSSProperties;
}) {
  return (
    <div className={cn("u-skel rounded-cell", className)} style={style} aria-hidden />
  );
}

/** 一列还没到的行。左栏、列表，任何「将来是一行一行」的地方都用它。
 *
 * 每行只有一条通栏的灰条：**不画图标位、不画右端、不模拟缩进**。那些都是此刻
 * 还不知道的东西，画出来就是编的——多一个灰方块，读者会当它是复选框或色点；
 * 假装一层缩进，等来的树要是形状不同，那一下比不缩进更晃。骨架能诚实说出口的
 * 只有两件事：**将来这里是一行一行的，每行多高**。
 *
 * 行本身就是真的那个 `Row`，所以行高、内边距、圆角跟真列表逐像素一致，内容
 * 落下来时一行不跳。`density` 要跟宿主列表给的那一档一样。
 *
 * （放在这里而不是 `Row` 后面，是为了让三个等待用的原件挨着；函数声明会提升，
 * 引用后面的 `Row` 没问题。） */
export function SkeletonRows({
  rows = 8,
  density = "list",
  className,
}: {
  /** 画几行。**够说明"这里将来是一列"就行**，不必铺满：右边那个转圈已经
   *  说了正在加载，一列灰条从头排到底只是一堵条纹墙。 */
  rows?: number;
  density?: RowDensity;
  className?: string;
}) {
  return (
    <div className={cn("u-rail-list", className)} aria-hidden>
      {Array.from({ length: rows }, (_, i) => (
        /* 灰条**直接当行的内容**，不再套一层"撑满行高"的壳。套了之后一行 30px
           里只有 10px 是条，上下各空 10px，一列看着比真列表松得多。现在一行
           就是 4 + 16 + 4：条厚得像一行字，行距也回到该有的样子 */
        <Row key={i} density={density} flush disabled>
          <Skeleton className="h-4 w-full" />
        </Row>
      ))}
    </div>
  );
}

/** 画布中间的等待记号。**盖在已经画好的东西上，不是替掉它们**——网格、缩放塔、
 *  静态图例都跟数据无关，先画出来，这一层只说中间那块还在路上。
 *
 * 比行内的转圈大一档（28）：它要在一整屏画布的正中被一眼看到，而不是挤在
 * 一行字旁边。`pointer-events-none` 让底下的控件照常能点——等的时候缩放、
 * 归位这些事本来就做得了。
 *
 * 宿主要有 `relative`（两张画布都是 `h-full relative`）。 */
export function CanvasLoading({ size = 28 }: { size?: number }) {
  return (
    <div className="absolute inset-0 grid place-items-center pointer-events-none">
      <Spinner size={size} label={S.nav.loading} />
    </div>
  );
}

/** 表格版的骨架行。
 *
 * 与 `SkeletonRows` 同一条道理，只是行的形状换成表的：用**真的 `Tr` / `Td`**
 * 搭，行高就由构造保证（8 + 22 + 8 = 38），不靠写死一个数。
 *
 * 这一条是踩出来的：表格的骨架一度直接用了左栏那一档的行（24px），比真表行
 * 矮了三分之一，一列排下来又矮又密，一眼就看得出不是那张表将来的样子。
 * 骨架说的是"等的是什么形状"，形状说错了就不如不说。 */
export function SkeletonTableRows({ rows = 8 }: { rows?: number }) {
  return (
    <SkelTable>
      <SkelTBody>
        {Array.from({ length: rows }, (_, i) => (
          <SkelTr key={i}>
            <SkelTd>
              {/* 这一层撑的是一个行高（见 styles.css 的 .u-skel-line）：
                  真单元格里是一行字，少了它整表矮一截 */}
              <span className="u-skel-line">
                <Skeleton className="h-4 w-full" />
              </span>
            </SkelTd>
          </SkelTr>
        ))}
      </SkelTBody>
    </SkelTable>
  );
}

/** 转圈。**形状未知的东西用它**：一整块区域还不知道会画成图、表还是空。
 *  形状已知的地方别用它——那里 `Skeleton` 能多说一句"等的是什么"。 */
export function Spinner({
  size = 16,
  label,
  className,
}: {
  size?: number;
  /** 读屏用；不给就整个当装饰藏起来 */
  label?: string;
  className?: string;
}) {
  return (
    <Loader2
      size={size}
      className={cn("animate-spin text-ink-2", className)}
      role={label ? "status" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    />
  );
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

/* ---------- PageHeader（栏右内容区的页级标题） ----------
   display 字号（规矩 1：页标题，也只有页标题）+ 一句副标题 + 右端的动作。
   每一页都从这里拿标题，字号、副标题的颜色、到正文的距离就不会各写各的。
   className 给了就替掉默认的 mb-6（外层已经用 space-y 排的地方传 mb-2 之类）。 */
export function PageHeader({
  title,
  sub,
  actions,
  className,
}: {
  title: ReactNode;
  sub?: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn(className ?? "mb-6")}>
      {/* 动作跟**标题那一行**走，不跟「标题 + 副标题」这一整块走：整块居中的话，
          有副标题时按钮就浮在两行中间，看着既不属于标题也不属于说明。
          items-baseline：按钮上的字与标题坐在同一条基线上 */}
      <div className="flex items-baseline justify-between gap-4">
        <h1 className="u-title min-w-0 text-display break-words">{title}</h1>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </div>
      {sub && <p className="mt-1 text-body text-ink-2">{sub}</p>}
    </div>
  );
}

/* ---------- SectionMark（分区字标：Docs/账户层等，逐字母入场，点击回应用） ---------- */
import { Link as RouterLink } from "@tanstack/react-router";
export function SectionMark({
  text,
  title,
  className,
}: {
  text: string;
  title: string;
  className?: string;
}) {
  return (
    <RouterLink
      to="/"
      title={title}
      className={cn("u-wordmark-top relative inline-flex text-ink", className)}
      style={{ fontFamily: "var(--font-brand)", letterSpacing: "0.01em" }}
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
export type RowDensity = "nav" | "list" | "menu";
/** 行的语义色：danger = 会删东西的那一行；warn = 通往危险区的入口——只是去往，
 *  还没动手，用警示色而不是危险色 */
export type RowTone = "danger" | "warn";
export function rowClass(
  active?: boolean,
  density: RowDensity = "list",
  tone?: RowTone,
  /** 二级：左栏里挂在某一项下面的那几条。文字缩到父行标签的位置
      （内距 12 + 图标 14 + 间距 8 = 34），底色仍然铺满整行——
      缩的是字，不是那一格 */
  sub?: boolean,
): string {
  return cn(
    "group flex w-full items-center gap-2 text-left transition-colors duration-fast",
    density === "menu" ? "rounded-none" : "rounded-cell",
    sub && "pl-[34px]",
    // 左栏导航项 32 高（py 6）：36 在一列十几条里显得松
    density === "nav"
      ? "px-3 py-1.5 text-body font-medium"
      : density === "menu"
        ? "px-3 py-2 text-small"
        : "px-2 py-1 text-body",
    active
      ? "u-nav-active"
      : tone === "danger"
        ? "text-danger hover:bg-surface-2"
        : tone === "warn"
          ? "text-warn hover:bg-surface-2"
          : "text-ink-2 hover:bg-surface-2 hover:text-ink",
  );
}
/** 行右端小字：小一档、淡一档，整行被指着时跟着提亮 */
export const ROW_TRAILING = "ml-auto shrink-0 text-fine text-ink-2 group-hover:text-ink";
/** 紧跟在标签后面的值——与 `ROW_TRAILING` 同一副颜色，但**不推到右边**。
 *  一左一右适合"名字 …… 数量"这种两栏读法；「谓词 宾语」是一句话，中间隔一
 *  整行空白就读不成句子了 */
export const ROW_VALUE =
  "min-w-0 flex-1 truncate text-fine text-ink-2 group-hover:text-ink";

export function Row({
  active,
  danger,
  tone,
  density = "list",
  indent = 0,
  icon,
  flush,
  trailing,
  className,
  children,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean;
  /** 危险的那一行（菜单里的删除）；= tone="danger" */
  danger?: boolean;
  tone?: RowTone;
  /** nav = 左栏导航项；list = 列表行；menu = 弹出菜单里的一项（顶满、不圆角） */
  density?: RowDensity;
  /** 树形缩进的层级 */
  indent?: number;
  icon?: ReactNode;
  /** 收掉图标那一格：整组行都没有图标时，那一格没有对齐对象，只是把标题往右推 */
  flush?: boolean;
  /** 右端的东西：计数、类型小字 */
  trailing?: ReactNode;
}) {
  return (
    <button
      type={type}
      aria-current={active ? "true" : undefined}
      style={indent ? { paddingLeft: `${8 + indent * 14}px` } : undefined}
      className={cn(rowClass(active, density, danger ? "danger" : tone), className)}
      {...props}
    >
      {/* 图标跟文字同色：选中变白、警示变橙都一起来。导航项没图标也留出
          图标那一格，一列里有图标的和没图标的文字对齐——除非整组都没有（flush） */}
      {icon ? (
        <span className="shrink-0">{icon}</span>
      ) : density === "nav" && !flush ? (
        <span className="w-3.5 shrink-0" aria-hidden />
      ) : null}
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {trailing && <span className={ROW_TRAILING}>{trailing}</span>}
    </button>
  );
}

/* 左栏的一条入口：Library 的来源、Review 的队列、Ontology 底部钉住的那几个
   都是它。**与列表里的行同一副样子**（Row density="nav"，圆角、缩进）——
   钉在底部的靠外面那条分隔线说明「这几个是常驻的」，不靠把行本身画成方的。
   计数给了就显示，0 也显示（灰一档）：队列清空了是个有意义的事实。 */
/** 一条会话此刻站在哪儿，画在左栏行的图标格里（14 宽，与别处的图标同一格，
 *  所以标题仍落在同一条竖线上，见 rowClass 的说明）。
 *
 *  三档：`rest` 6px 细线空心圆——它本身也是个记号，说明这一列每一行都是一场对话；
 *  `live` 三个点在跳；`unread` 同一个圆填成实心蓝，写完了而你还没回来看。
 *  **形状一档不变**，只有里外与颜色在变：换状态不换形状，余光里才认得出是
 *  同一样东西。小到只起项目符号的作用，不跟标题抢眼（从前 12 的方块太重） */
export function ConvMark({ state }: { state: "rest" | "live" | "unread" }) {
  return (
    // `flex` 不是 `inline-flex`：Row 把图标包在一个 span 里，inline 的盒会坐在
    // 那一行字的基线上、底下还垫着行高，于是整个记号偏下（三个点里没有字，
    // 基线就是底边，偏得最明显）。块级的盒没有行框，交给 Row 的 items-center 居中
    <span
      className="flex h-3.5 w-3.5 shrink-0 items-center justify-center"
      aria-hidden
    >
      {state === "live" ? (
        <span className="u-dots">
          <span />
          <span />
          <span />
        </span>
      ) : (
        <span
          className={cn(
            "h-1.5 w-1.5 rounded-full border",
            state === "unread" ? "border-unread bg-unread" : "border-ink-2",
          )}
        />
      )}
    </span>
  );
}

export function RailItem({
  active,
  icon,
  count,
  dot,
  external,
  className,
  children,
  ...props
}: Omit<ButtonHTMLAttributes<HTMLButtonElement>, "type"> & {
  active?: boolean;
  icon?: ReactNode;
  count?: number;
  /** 右侧的状态点（同步中/失败那种），给背景色的类名 */
  dot?: string;
  /** 这一条不在本页办——加个去向记号，免得点下去以为页面没反应 */
  external?: boolean;
}) {
  return (
    <Row
      density="nav"
      active={active}
      icon={icon}
      className={className}
      // 计数只在有东西时出现，写成一枚 chip；0 不写——一栏灰零只是噪音
      trailing={
        count !== undefined && count > 0 ? (
          <span className={chipLike("neutral", "u-num")}>{count}</span>
        ) : undefined
      }
      {...props}
    >
      <span className="flex min-w-0 items-center gap-2">
        <span className="truncate">{children}</span>
        {dot && <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", dot)} />}
        {external && <ArrowUpRight size={11} className="shrink-0 opacity-50" />}
      </span>
    </Row>
  );
}

/** 菜单里的一行「标签 + 可改的值」。**整行是触发器，弹层落在值那一头**——
 *  这一行读起来是「语言：English」，要改的是冒号后面那一格，弹层就该出现在
 *  那一格上，像填空。摊在下面会把菜单越拉越长，甩到右边又会盖住旁边的面板。
 *  行的节奏与菜单里其余的行一致（px-4 py-2，正文号，图标间距 3）。 */
const MENU_ROW = "gap-3 rounded-none px-4 py-2 text-body";

export function MenuSelect({
  icon,
  label,
  value,
  onChange,
  options,
}: {
  icon: ReactNode;
  label: string;
  value: string;
  onChange: (v: string) => void;
  /** 选项自己的图标是**可选的**：主题那三档各有公认的符号（月亮 / 太阳 /
   *  一块屏），图标一眼就把三个选项分开了。语言没有这种符号——国旗不是语言，
   *  说中文的不止一个地方——那一档就只有字，对勾负责说选中的是哪个。 */
  options: { value: string; label: string; icon?: ReactNode }[];
}) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger
        className={cn(
          MENU_ROW,
          // 行的样子归菜单：去掉 Select 自带的框与底，留下"一行"
          "h-auto w-full border-0 bg-transparent shadow-none focus-visible:ring-0 dark:bg-transparent",
          "justify-start text-ink",
        )}
      >
        <span className="text-ink-2">{icon}</span>
        <span>{label}</span>
        <span className="ml-auto pl-2 text-fine text-ink-2">
          {options.find((o) => o.value === value)?.label ?? value}
        </span>
      </SelectTrigger>
      {/* **popper 定位，不是 item-aligned。**Select 缺省是把"选中的那一项"盖在
          触发器上（一个原生 select 的样子）；在这张菜单里它算出来的位置是
          (0, 900)——视口外的左下角，于是弹层开了却看不见（`data-state=open`、
          内容也挂上了，就是不在屏幕上）。item-aligned 那套要量触发器，而触发器
          在一层 portal 里；popper 走 floating-ui，逐帧跟着触发器，也才认 align。 */}
      <SelectContent
        position="popper"
        side="bottom"
        align="end"
        sideOffset={4}
        /* **弹层按内容宽，不按触发器宽。**触发器是整行，而 shadcn 的 viewport 在
           popper 模式下写死 `min-w-(--radix-select-trigger-width)`，于是弹层跟着
           撑满整条菜单，看着像菜单自己长出来两行。它该落在值那一格上，像一个填空。
           改的是 `--radix-popper-anchor-width` 而不是 `--radix-select-trigger-width`：
           后者是 Radix 写在内容元素**内联样式**上的（`var(--radix-popper-anchor-width)`），
           class 压不过内联，只能改它引的那一格。 */
        className="[--radix-popper-anchor-width:auto]"
      >
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value}>
            {o.icon && <span className="text-ink-2">{o.icon}</span>}
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
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
  disabled,
  className,
}: {
  value: T;
  options: { value: T; label: ReactNode; count?: number; title?: string }[];
  onChange: (v: T) => void;
  size?: "sm" | "md";
  fill?: boolean;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <div
      role="tablist"
      className={cn(
        "flex gap-1 rounded-control bg-surface p-1",
        fill && "w-full",
        disabled && "opacity-40",
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
            disabled={disabled}
            onClick={() => onChange(o.value)}
            className={cn(
              "flex items-center justify-center gap-1 rounded-control font-medium transition-colors duration-fast",
              size === "sm" ? "px-2 py-1 text-fine" : "px-3 py-1 text-small",
              fill && "flex-1",
              active
                ? "bg-surface-3 text-ink"
                : "text-ink-2 hover:bg-surface-2 hover:text-ink",
            )}
          >
            {o.label}
            {o.count !== undefined && o.count > 0 && (
              <span className="u-num">{o.count}</span>
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
  checked,
  onChange,
  disabled,
  id,
}: {
  label: ReactNode;
  hint?: ReactNode;
  className?: string;
  checked?: boolean;
  /** **收的是布尔，不是事件**：底下已经不是原生 input 了（Radix 的按钮 +
   *  一个隐藏的输入），事件对象里的 `target.checked` 在这里没有意义 */
  onChange?: (checked: boolean) => void;
  disabled?: boolean;
  id?: string;
}) {
  const auto = useId();
  const inputId = id ?? auto;
  return (
    <div className={cn("flex items-start gap-2", className)}>
      <ShadCheckbox
        id={inputId}
        checked={checked}
        disabled={disabled}
        onCheckedChange={(v) => onChange?.(v === true)}
        className="mt-0.5"
      />
      <label htmlFor={inputId} className="min-w-0 cursor-pointer">
        <span className="block text-body text-ink">{label}</span>
        {hint && <span className="block text-fine text-ink-2">{hint}</span>}
      </label>
    </div>
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
      <summary className="cursor-pointer select-none text-small text-ink-2 transition-colors duration-fast hover:text-ink">
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
        "u-tower group glass-strong flex flex-col overflow-hidden rounded-panel u-lift-strong",
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

/* ---------- GroupLabel（一组内容的小标题） ----------
   小号、中等字重、句首大写。**不用大写字母拉字距**——那种小节标题看着像
   另一套字体系统，用户明确不要。停靠面板里的分段、抽取栏、总览的三段、
   左栏的分组都是它；右边可带计数，左边可带一个小图标（图谱面板里的方向箭头）。 */
export function GroupLabel({
  icon,
  count,
  tone = "default",
  className,
  children,
}: {
  icon?: ReactNode;
  count?: ReactNode;
  /** contest：被挡住的那一组，用争议色 */
  tone?: "default" | "contest";
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        "flex items-center gap-2 text-small font-medium",
        tone === "contest" ? "text-contest" : "text-ink-2",
        className,
      )}
    >
      {icon}
      <span className="min-w-0 truncate">{children}</span>
      {count !== undefined && <span className="u-num">{count}</span>}
    </div>
  );
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
/* ---------- RadioGroup（一组互斥的选项） ----------
   **一组是一个组件，不是一堆各自为政的 Radio**：方向键在组内移动、焦点只落
   在选中的那一个上、读屏器把它们念成一组——这些是"组"的性质，Radix 的
   RadioGroup 管着；从前每个 Radio 都是一个裸的 input，靠同名 `name` 凑成一组，
   键盘那部分只能听浏览器的默认行为。
   `children` 是选中之后跟在标签后面的东西（比如一个日期框）。 */
export function RadioGroup<T extends string>({
  value,
  onChange,
  options,
  className,
  name,
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: ReactNode; children?: ReactNode }[];
  className?: string;
  name?: string;
}) {
  return (
    <ShadRadioGroup
      value={value}
      onValueChange={(v) => onChange(v as T)}
      name={name}
      className={cn("gap-2", className)}
    >
      {options.map((o) => (
        <div key={o.value} className="flex items-center gap-2 text-small text-ink-2">
          <ShadRadioGroupItem value={o.value} id={`${name ?? "radio"}-${o.value}`} />
          <label htmlFor={`${name ?? "radio"}-${o.value}`} className="cursor-pointer">
            {o.label}
          </label>
          {value === o.value && o.children}
        </div>
      ))}
    </ShadRadioGroup>
  );
}

/* ---------- ExpandCard（可展开的一条：事实、派生、年表条目） ----------
   折叠行只有一副样子：左边固定一格 chevron（开了转 90°），右边是头——一行
   或两行文字——展开的内容缩进到文字底下（pl-7 = 内距 8 + chevron 12 + 间距 8）。
   头是一整条可点的按钮，里面可以有 role="link" 的 span（去看另一端），
   但不能有按钮——按钮里不能嵌按钮。 */
export function ExpandCard({
  open,
  onToggle,
  dim,
  title,
  className,
  header,
  children,
}: {
  open: boolean;
  onToggle: () => void;
  /** 陈旧的那种：整条压淡 */
  dim?: boolean;
  title?: string;
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
        className="flex w-full items-start gap-2 px-2 py-1.5 text-left"
      >
        <ChevronRight
          size={12}
          className={cn("mt-1 shrink-0 text-ink-2 u-turn", open && "rotate-90")}
        />
        <div className="min-w-0 flex-1">{header}</div>
      </button>
      {children && <div className="pb-2 pl-7 pr-2">{children}</div>}
    </div>
  );
}

/** 面板内一行的悬停。**只给状态，不给布局**——每种列表的行高与内边距不同，
    而悬停是同一件事。面板本身不响应指针（DESIGN.md 6），悬停归行所有。 */
export const ROW_HOVER = "transition-colors duration-fast hover:bg-surface-2";

/** 复合行的外壳：一行里有两个按钮时不能是 Row（按钮里不能嵌按钮），
    外层 div 用它拿到 hover 与 group */
export const HOVER_ROW =
  "group flex items-center gap-2 rounded-cell px-2 py-1 transition-colors duration-fast hover:bg-surface-2";
/** 指针停在所在行（.group）上才现身的东西；加 is-on 常显 */
export const REVEAL = "u-reveal";

/* 各在自己文件里的组件，从这里一并导出，页面只认 "../ui" 一个入口 */
export { Dialog, DangerConfirm, FormDialog } from "./dialog";
export { Tooltip } from "./tooltip";
export { Table, THead, TBody, Tr, Th, Td } from "./table";
export { Field } from "./field";
