import { expect, it } from "vitest";
import type { Kb } from "../api";
import { kbGroups } from "./kbGroups";

const kb = (name: string) => ({ id: name, name }) as Kb;

it("keeps bases without a slash as plain entries", () => {
  const mercedes = kb("mercedes");
  expect(kbGroups([mercedes])).toEqual([{ kind: "kb", kb: mercedes }]);
});

it("groups bases sharing the text before the slash under that name", () => {
  const [lore, floor] = [kb("re-cinq/lore"), kb("re-cinq/floor")];
  expect(kbGroups([lore, floor])).toEqual([
    {
      kind: "group",
      name: "re-cinq",
      items: [
        { kb: lore, label: "lore" },
        { kb: floor, label: "floor" },
      ],
    },
  ]);
});

it("places a group where its first member was and splits only on the first slash", () => {
  const [a, lore, b, deep, floor] = [kb("alpha"), kb("re-cinq/lore"), kb("beta"), kb("x/y/z"), kb("re-cinq/floor")];
  expect(kbGroups([a, lore, b, deep, floor])).toEqual([
    { kind: "kb", kb: a },
    { kind: "group", name: "re-cinq", items: [{ kb: lore, label: "lore" }, { kb: floor, label: "floor" }] },
    { kind: "kb", kb: b },
    { kind: "group", name: "x", items: [{ kb: deep, label: "y/z" }] },
  ]);
});
