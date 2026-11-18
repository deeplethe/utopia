import { expect, it } from "vitest";
import { withGroupHeadings } from "./withGroupHeadings";

const lore = { value: "1", label: "re-cinq/lore", group: "re-cinq", short: "lore" };
const floor = { value: "2", label: "re-cinq/floor", group: "re-cinq", short: "floor" };
const mercedes = { value: "3", label: "mercedes" };

it("puts one heading before each run of a group and none before ungrouped options", () => {
  expect(withGroupHeadings([mercedes, lore, floor])).toEqual([
    { kind: "option", option: mercedes, index: 0 },
    { kind: "heading", name: "re-cinq" },
    { kind: "option", option: lore, index: 1 },
    { kind: "option", option: floor, index: 2 },
  ]);
});
