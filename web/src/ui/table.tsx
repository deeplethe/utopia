/* 表格：只管皮。表头 fine 号大写字距，行间细线，指针停上一行变 surface-2。
   数据列的对齐、宽度由页面决定；这里不做排序、不做分页（Pager 在 index.tsx）。 */
import type {
  HTMLAttributes,
  TableHTMLAttributes,
  TdHTMLAttributes,
  ThHTMLAttributes,
} from "react";
import { cn } from "./index";

export function Table({
  className,
  ...props
}: TableHTMLAttributes<HTMLTableElement>) {
  return (
    <div className="w-full overflow-x-auto">
      <table
        className={cn("w-full border-collapse text-body text-ink", className)}
        {...props}
      />
    </div>
  );
}

export function THead({
  className,
  ...props
}: HTMLAttributes<HTMLTableSectionElement>) {
  return <thead className={cn("text-left", className)} {...props} />;
}

export function TBody({
  className,
  ...props
}: HTMLAttributes<HTMLTableSectionElement>) {
  return <tbody className={className} {...props} />;
}

export function Tr({
  interactive,
  className,
  ...props
}: HTMLAttributes<HTMLTableRowElement> & {
  /** 可点的行：指针停上变面，光标变手 */
  interactive?: boolean;
}) {
  return (
    <tr
      className={cn(
        "border-b border-line",
        interactive && "cursor-pointer transition-colors duration-fast hover:bg-surface-2",
        className,
      )}
      {...props}
    />
  );
}

export function Th({
  className,
  ...props
}: ThHTMLAttributes<HTMLTableCellElement>) {
  return (
    <th
      className={cn(
        "border-b border-line-strong px-3 py-2 text-fine font-medium uppercase tracking-wider text-ink-3",
        className,
      )}
      {...props}
    />
  );
}

export function Td({
  className,
  ...props
}: TdHTMLAttributes<HTMLTableCellElement>) {
  return <td className={cn("px-3 py-2 align-top", className)} {...props} />;
}
