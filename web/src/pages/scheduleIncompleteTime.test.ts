import { Children, isValidElement, type ReactNode } from "react";
import { expect, it, vi } from "vitest";
const state = vi.hoisted(() => ({ time: "09:" }));
vi.mock("react", async (original) => ({ ...(await original<typeof import("react")>()), useState: (initial: unknown) => [initial === "09:00" ? state.time : typeof initial === "function" ? initial() : initial, vi.fn()] }));
import { SchedulePicker } from "./Library";
type Props = { children?: ReactNode; onClick?: () => void };
function dayButton(node: ReactNode): Props | undefined {
  if (!isValidElement<Props>(node)) return;
  if (node.props.children === "Tue") return node.props;
  return Children.toArray(node.props.children).map(dayButton).find(Boolean);
}
it.each(["09:", "25:00", ""])("does not emit a day change while the time is %s", (time) => {
  state.time = time;
  const onChange = vi.fn();
  const button = dayButton(SchedulePicker({ initial: { sync_cron: "0 9 * * Mon", sync_interval_minutes: null }, onChange }));
  button!.onClick!();
  expect(onChange).not.toHaveBeenCalled();
});
it("still emits a valid time when weekdays change", () => {
  state.time = "09:00";
  const onChange = vi.fn();
  dayButton(SchedulePicker({ initial: { sync_cron: "0 9 * * Mon", sync_interval_minutes: null }, onChange }))!.onClick!();
  expect(onChange).toHaveBeenCalledWith({ sync_cron: "0 9 * * Mon,Tue", sync_interval_minutes: null });
});
