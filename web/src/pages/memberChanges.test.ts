import { expect, it } from "vitest";
import { memberChanges } from "./memberChanges";

const member = { role: "editor", is_admin: false };

it("returns nothing when neither role nor system admin changed", () => {
  expect(memberChanges(member, { role: "editor", is_admin: false })).toEqual({});
});

it("returns is_admin true when only system admin was switched on", () => {
  expect(memberChanges(member, { role: "editor", is_admin: true })).toEqual({ is_admin: true });
});

it("returns role and is_admin when both changed", () => {
  expect(memberChanges(member, { role: "owner", is_admin: true })).toEqual({ role: "owner", is_admin: true });
});
