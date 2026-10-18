import { expect, it } from "vitest";
import { groupRowKey, submenuKey } from "./kbMenuKeys";

it.each(["Enter", " ", "ArrowRight"])("opens the submenu on the group row's %j key", (key) => {
  expect(groupRowKey(key)).toBe("open");
});

it.each(["ArrowLeft", "ArrowDown", "Tab", "a"])("leaves the group row's %j key alone", (key) => {
  expect(groupRowKey(key)).toBeUndefined();
});

it.each([
  ["Escape", "close"],
  ["ArrowLeft", "close"],
  ["ArrowDown", 1],
  ["ArrowUp", -1],
  ["Enter", undefined],
] as const)("maps %j inside the submenu to %j", (key, action) => {
  expect(submenuKey(key)).toBe(action);
});
