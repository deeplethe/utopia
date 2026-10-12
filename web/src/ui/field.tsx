/* 表单项：标签 + 控件 + 一句提示或一句错误。间距在这里定一次，
   页面里不再有 mb-3 / mt-1 各写各的。 */
import { cloneElement, isValidElement, useId, type ReactNode } from "react";
import { cn, Input, Textarea } from "./index";

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
  const generatedId = useId();
  const textControl = isValidElement<{ id?: string }>(children) &&
    (children.type === Input || children.type === Textarea || children.type === "input" || children.type === "textarea");
  // Most callers omit htmlFor; keep the visible label usable for their text control.
  const controlId = textControl ? children.props.id ?? htmlFor ?? generatedId : undefined;
  const control = textControl && !children.props.id ? cloneElement(children, { id: controlId }) : children;
  return (
    <div className={cn("mb-4", className)}>
      <label htmlFor={htmlFor ?? controlId} className="mb-1 block text-small font-medium text-ink-2">
        {label}
      </label>
      {control}
      {(error || hint) && (
        <p className={cn("mt-1 text-fine", error ? "text-danger" : "text-ink-2")}>
          {error ?? hint}
        </p>
      )}
    </div>
  );
}
