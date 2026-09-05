/* 表单项：标签 + 控件 + 一句提示或一句错误。间距在这里定一次，
   页面里不再有 mb-3 / mt-1 各写各的。 */
import type { ReactNode } from "react";
import { cn } from "./index";

export function Field({
  label,
  hint,
  error,
  htmlFor,
  className,
  children,
}: {
  label: ReactNode;
  /** 控件下面的一句说明 */
  hint?: ReactNode;
  /** 有错误时替掉说明，用危险色 */
  error?: ReactNode;
  htmlFor?: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={cn("mb-4", className)}>
      <label htmlFor={htmlFor} className="mb-1 block text-small text-ink-2">
        {label}
      </label>
      {children}
      {(error || hint) && (
        <p className={cn("mt-1 text-fine", error ? "text-danger" : "text-ink-3")}>
          {error ?? hint}
        </p>
      )}
    </div>
  );
}
