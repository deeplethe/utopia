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
