import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { createServer } from "vite";

// Run with the optional Chromium setup used by chat-pagination.test.mjs.
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.CHAT_PLAYWRIGHT_PATH || "playwright-core");
const root = fileURLToPath(new URL("../", import.meta.url));
const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createRouter, createRootRoute, createRoute, RouterProvider, Outlet } from '@tanstack/react-router';
import { KbSettings } from '/src/pages/KbSettings.tsx';
const parent = createRootRoute({ component: Outlet });
const app = createRoute({ getParentRoute: () => parent, id: 'app', component: Outlet });
const kb = createRoute({ getParentRoute: () => app, path: '/kb/$kbId', component: Outlet });
const settingsRoute = createRoute({ getParentRoute: () => kb, path: 'settings',
  component: KbSettings });
const router = createRouter({ routeTree: parent.addChildren([app.addChildren([kb.addChildren([settingsRoute])])]) });
const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
createRoot(document.getElementById('root')).render(
  React.createElement(QueryClientProvider, { client }, React.createElement(RouterProvider, { router })),
);
`;
const fixture = {
  name: "activity-ui-fixture",
  enforce: "pre",
  resolveId(id) {
    if (id === "/activity-ui-entry.js") return "\0activity-ui-entry";
  },
  load(id) {
    if (id === "\0activity-ui-entry") return entry;
  },
  configureServer(server) {
    server.middlewares.use(async (req, res, next) => {
      if (!req.url?.startsWith("/kb/")) return next();
      try {
        res.setHeader("content-type", "text/html");
        res.end(await server.transformIndexHtml(req.url,
          '<html><body><div id="root"></div><script type="module" src="/activity-ui-entry.js"></script></body></html>'));
      } catch (error) {
        next(error);
      }
    });
  },
};

const recorded = {
  id: "event",
  actor_id: "person",
  actor_name: "Alice Example",
  action: "entity.created",
  target_kind: "entity",
  target_id: "example",
  detail: {},
  created_at: "2026-11-14T00:00:00Z",
};
const settings = {
  id: "one",
  name: "one",
  description: null,
  visibility: "open",
  is_default: true,
  materialize_inferences: false,
  auto_type_resolution: false,
  governance: false,
  inference_interval_minutes: 60,
  ontology_lang: "en",
};

function filteredEvents(filtered) {
  if (!filtered) return [recorded];
  return Array.from({ length: 51 }, (_, i) => ({
    ...recorded,
    id: `event-${i}`,
    actor_name: i === 50 ? "Second-page Editor" : `First-page Editor ${i + 1}`,
  }));
}

test("Activity retries a 503 without losing filters or pagination", { timeout: 45_000 }, async (t) => {
  const server = await createServer({
    root, configFile: false, plugins: [fixture],
    resolve: { alias: { "@": `${root}src` } }, esbuild: { jsx: "automatic" },
    optimizeDeps: { include: ["react/jsx-dev-runtime"] },
    server: { host: "127.0.0.1", port: 0, hmr: false },
  });
  await server.listen();
  t.after(() => server.close());
  const browser = await chromium.launch({ executablePath: process.env.CHAT_CHROMIUM_PATH || undefined, headless: true });
  t.after(() => browser.close());
  const page = await browser.newPage({ timezoneId: "UTC" });
  page.setDefaultTimeout(8_000);
  const errors = [];
  page.on("pageerror", (err) => errors.push(err.message));
  const requests = [];
  let failFirstRequest = true;
  let failSecondPage = true;
  let resumeResponse;
  const delayedResponse = new Promise((resolve) => { resumeResponse = resolve; });
  const newSince = "2026-11-02T00:00:00.000Z";
  await page.route("**/api/**", async (route) => {
    const { pathname, searchParams } = new URL(route.request().url());
    if (pathname === "/api/v1/kbs/one") return route.fulfill({ json: settings });
    if (pathname === "/api/v1/kbs/one/jobs/failed") return route.fulfill({ json: { failed: 0 } });
    if (pathname === "/api/v1/kbs/one/audit") {
      const request = Object.fromEntries(searchParams);
      requests.push(request);
      const { action, since, until, limit, offset } = request;
      if (failFirstRequest) {
        failFirstRequest = false;
        return route.fulfill({ status: 503, json: { error: "Audit storage is temporarily unavailable." } });
      }
      const filtered = action === recorded.action && since === "2026-11-01T00:00:00.000Z" &&
        until === "2026-12-01T00:00:00.000Z";
      if (filtered && offset === "50" && failSecondPage) {
        failSecondPage = false;
        return route.fulfill({ status: 503, json: { error: "Filtered audit request failed." } });
      }
      if (since === newSince) await delayedResponse;
      const all = since?.startsWith("2030-") ? [] : filteredEvents(filtered);
      const events = all.slice(Number(offset), Number(offset) + Number(limit));
      return route.fulfill({ json: { events, total: all.length, actions: [recorded.action] } });
    }
    errors.push(`Unexpected API path ${pathname}`);
    return route.fulfill({ status: 500, json: { error: "Unexpected API path" } });
  });
  const exact = (text) => page.getByText(text, { exact: true });
  const empty = exact("Nothing recorded yet.");
  const zero = exact("0 events");
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/kb/one/settings`);
  const response = page.waitForResponse((r) => new URL(r.url()).pathname.endsWith("/audit"));
  await exact("Activity").click();
  assert.equal((await response).status(), 503);
  const notice = page.getByRole("alert");
  await notice.or(empty).waitFor();
  assert.equal(await notice.count(), 1, "A 503 must show an error instead of an empty ledger");
  assert.match(await notice.innerText(), /Could not load the activity log/);
  assert.match(await notice.innerText(), /Audit storage is temporarily unavailable/);
  assert.equal(await empty.count(), 0);
  assert.equal(await zero.count(), 0);
  const retry = page.getByRole("button", { name: "Retry", exact: true });
  await retry.click();
  await exact("Alice Example").waitFor();
  const date = page.getByTitle("From this date");
  const until = page.getByTitle("Up to this date");
  await page.getByRole("button", { name: "All actions", exact: true }).click();
  await page.getByRole("menuitemradio", { name: recorded.action, exact: true }).click();
  await date.fill("2026-11-01");
  await until.fill("2026-11-30");
  await exact("First-page Editor 1").waitFor();
  const next = exact("1–50 of 51").locator("..").getByRole("button").last();
  await next.click();
  await notice.or(empty).waitFor();
  assert.equal(await notice.count(), 1, "A 503 on the second page must show an error");
  assert.match(await notice.innerText(), /Filtered audit request failed/);
  assert.equal(await empty.count(), 0);
  assert.equal(await zero.count(), 0);
  const expected = {
    action: recorded.action,
    since: "2026-11-01T00:00:00.000Z",
    until: "2026-12-01T00:00:00.000Z",
    limit: "50",
    offset: "50",
  };
  assert.deepEqual(requests.at(-1), expected);
  const selectedAction = page.getByRole("button", { name: recorded.action, exact: true });
  assert.equal(await selectedAction.count(), 1);
  const attempts = requests.length;
  await retry.click();
  await exact("Second-page Editor").waitFor();
  await exact("51–51 of 51").waitFor();
  assert.equal(await date.inputValue(), "2026-11-01");
  assert.equal(await until.inputValue(), "2026-11-30");
  assert.equal(await selectedAction.count(), 1);
  assert.deepEqual(requests.slice(attempts), [expected]);
  assert.equal(await notice.count(), 0);
  await date.fill("2030-11-01");
  await empty.waitFor();
  await zero.waitFor();
  assert.equal(await notice.count(), 0);
  assert.deepEqual(requests.at(-1), { ...expected, since: "2030-11-01T00:00:00.000Z", offset: "0" });
  const request = page.waitForRequest((req) => new URL(req.url()).searchParams.get("since") === newSince);
  try {
    await date.fill("2026-11-02");
    await request;
    await page.getByRole("status").waitFor();
    assert.equal(await empty.count(), 0);
    assert.equal(await zero.count(), 0);
  } finally {
    resumeResponse();
  }
  await exact("Alice Example").waitFor();
  assert.deepEqual(errors, []);
});
