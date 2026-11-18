import { expect, it } from "vitest";
import { loginSections, ssoErrorKey } from "./loginSections";

it("shows the password form and the SSO button when both are on", () => {
  expect(loginSections({ enabled: true, password_login: true })).toEqual({ password: true, sso: true });
});

it("shows only the SSO button when password sign-in is off", () => {
  expect(loginSections({ enabled: true, password_login: false })).toEqual({ password: false, sso: true });
});

it("keeps the password form when SSO is not configured, even with password sign-in off", () => {
  expect(loginSections({ enabled: false, password_login: false })).toEqual({ password: true, sso: false });
});

it("shows the password form while the status is still loading", () => {
  expect(loginSections(undefined)).toEqual({ password: true, sso: false });
});

it("words an unlinked SSO error without the password hint when the form is hidden", () => {
  expect([ssoErrorKey("unlinked", false), ssoErrorKey("unlinked", true), ssoErrorKey("taken", false)]).toEqual([
    "unlinked_sso_only",
    "unlinked",
    "taken",
  ]);
});
