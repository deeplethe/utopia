import { Children, isValidElement, type ReactNode } from "react";
import { expect, it, vi } from "vitest";
vi.mock("../i18n", async (original) => ({ ...(await original<typeof import("../i18n")>()), S: (await import("../i18n/zh")).zh }));
vi.mock("react", async (original) => ({ ...(await original<typeof import("react")>()), useState: (initial: unknown) => [typeof initial === "function" ? initial() : initial, vi.fn()] }));
import { SchedulePicker, scheduleToPickerState } from "./Library";
type Props = { children?: ReactNode; onClick?: () => void };
function dayButton(node: ReactNode): Props | undefined {
  if (!isValidElement<Props>(node)) return;
  if (node.props.children === "周二") return node.props;
  return Children.toArray(node.props.children).map(dayButton).find(Boolean);
}
it("reads English cron weekdays in a Chinese interface", () => {
  const state = scheduleToPickerState({ sync_cron: "0 9 * * Mon", sync_interval_minutes: null });
  expect(state.mode).toBe("weekly");
  expect([...state.days]).toEqual([0]);
});
it("emits cron weekday names independently of display labels", () => {
  const onChange = vi.fn();
  const tree = SchedulePicker({ initial: { sync_cron: "0 9 * * Mon", sync_interval_minutes: null }, onChange });
  const button = dayButton(tree);
  expect(button?.onClick).toBeTypeOf("function");
  button!.onClick!();
  expect(onChange).toHaveBeenCalledWith({ sync_cron: "0 9 * * Mon,Tue", sync_interval_minutes: null });
});
