import { Children, isValidElement, type ChangeEvent, type ReactNode } from "react";
import { expect, it, vi } from "vitest";
vi.mock("react", async (original) => ({ ...(await original<typeof import("react")>()), useState: (initial: unknown) => [typeof initial === "function" ? initial() : initial, vi.fn()] }));
import { SchedulePicker } from "./Library";
type Props = { children?: ReactNode; type?: string; onChange?: (e: ChangeEvent<HTMLInputElement>) => void };
function numberInput(node: ReactNode): Props | undefined {
  if (!isValidElement<Props>(node)) return;
  if (node.props.type === "number") return node.props;
  return Children.toArray(node.props.children).map(numberInput).find(Boolean);
}
it.each([[5, 3], [120, 210]])("submits integer minutes for an initial interval of %s", (initial, expected) => {
  const onChange = vi.fn();
  const input = numberInput(SchedulePicker({ initial: { sync_interval_minutes: initial, sync_cron: null }, onChange }));
  input!.onChange!({ target: { value: "3.5" } } as ChangeEvent<HTMLInputElement>);
  expect(onChange).toHaveBeenCalledWith({ sync_interval_minutes: expected, sync_cron: null });
});
