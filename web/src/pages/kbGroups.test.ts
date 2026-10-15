import { expect, it } from "vitest";
import type { Kb } from "../api";
import { kbGroups } from "./kbGroups";

const kb = (name: string) => ({ id: name, name }) as Kb;

it("keeps bases without a slash as plain entries", () => {
  const mercedes = kb("mercedes");
  expect(kbGroups([mercedes])).toEqual([{ kind: "kb", kb: mercedes }]);
});
