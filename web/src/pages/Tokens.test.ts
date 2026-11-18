import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterAll, expect, it, vi } from "vitest";
import type { TokenView } from "../api";
import { IssuedBody } from "./Tokens";

vi.stubGlobal("window", { location: { origin: "https://utopia.test" } });
afterAll(() => vi.unstubAllGlobals());

const kbs = [{ id: "general", name: "General" }, { id: "research", name: "Research" }];
const info: TokenView = {
  id: "token", name: "Agent", token_prefix: "utp_pat_test", scope: "read", kb_ids: null,
  created_at: "2026-10-21T00:00:00Z", expires_at: null, last_used_at: null, revoked_at: null,
};

it.each([
  { allowed: null, expected: "research" },
  { allowed: ["general", "research"], expected: "research" },
  { allowed: ["general"], expected: "general" },
])("defaults the copied endpoint to a permitted last-opened base ($allowed)", ({ allowed, expected }) => {
  const html = renderToStaticMarkup(createElement(IssuedBody, {
    token: "utp_pat_test", info: { ...info, kb_ids: allowed }, kbs, preferredKbId: "research",
  }));
  expect(html).toContain(`https://utopia.test/api/v1/kbs/${expected}/mcp`);
});
