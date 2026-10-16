import { expect, it } from "vitest";
import type { Kb } from "../api";
import { kbGroups, kbOptions } from "./kbGroups";

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
  const [a, xy, b, xyz] = [kb("alpha"), kb("x/y"), kb("beta"), kb("x/y/z")];
  expect(kbGroups([a, xy, b, xyz])).toEqual([
    { kind: "kb", kb: a },
    { kind: "group", name: "x", items: [{ kb: xy, label: "y" }, { kb: xyz, label: "y/z" }] },
    { kind: "kb", kb: b },
  ]);
});

it("keeps CI/CD a plain row when no other base shares the CI prefix", () => {
  const [cicd, mercedes] = [kb("CI/CD"), kb("mercedes")];
  expect(kbGroups([cicd, mercedes])).toEqual([
    { kind: "kb", kb: cicd },
    { kind: "kb", kb: mercedes },
  ]);
});

it("keeps /foo, foo/ and / plain rows because one side of the slash is empty", () => {
  const names = ["/foo", "/bar", "foo/", "foo/ ", "/", " /x", " /y"].map(kb);
  expect(kbGroups(names)).toEqual(names.map((k) => ({ kind: "kb", kb: k })));
});

it("puts re-cinq /lore and re-cinq/floor in one re-cinq group with trimmed labels", () => {
  const [lore, floor] = [kb("re-cinq /lore"), kb("re-cinq/ floor")];
  expect(kbGroups([lore, floor])).toEqual([
    { kind: "group", name: "re-cinq", items: [{ kb: lore, label: "lore" }, { kb: floor, label: "floor" }] },
  ]);
});

it("gives a lone A/B tests base a plain option with its full name", () => {
  expect(kbOptions([kb("A/B tests")])).toEqual([{ value: "A/B tests", label: "A/B tests" }]);
});

it("turns bases into select options with the group's members side by side", () => {
  const [lore, mercedes, floor] = [kb("re-cinq/lore"), kb("mercedes"), kb("re-cinq/floor")];
  expect(kbOptions([lore, mercedes, floor])).toEqual([
    { value: "re-cinq/lore", label: "re-cinq/lore", group: "re-cinq", short: "lore" },
    { value: "re-cinq/floor", label: "re-cinq/floor", group: "re-cinq", short: "floor" },
    { value: "mercedes", label: "mercedes" },
  ]);
});
