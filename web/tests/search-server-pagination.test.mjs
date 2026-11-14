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
import { Search } from '/src/pages/Search.tsx';
const parent = createRootRoute({ component: Outlet });
const app = createRoute({ getParentRoute: () => parent, id: 'app', component: Outlet });
const kb = createRoute({ getParentRoute: () => app, path: '/kb/$kbId', component: Outlet });
const search = createRoute({ getParentRoute: () => kb, path: 'search',
  validateSearch: ({q}) => ({q}), component: Search });
const doc = createRoute({ getParentRoute: () => kb, path: 'doc/$docId', component: () => null });
const router = createRouter({ routeTree: parent.addChildren([app.addChildren([kb.addChildren([search, doc])])]) });
const client = new QueryClient({ defaultOptions: {
  queries: { retry: false, staleTime: 15_000, refetchOnWindowFocus: false },
} });
createRoot(document.getElementById('root')).render(
  React.createElement(QueryClientProvider, { client }, React.createElement(RouterProvider, { router })),
);
`;
const fixture = {
  name: "search-ui-fixture",
  enforce: "pre",
  resolveId(id) {
    if (id === "/search-ui-entry.js") return "\0search-ui-entry";
  },
  load(id) {
    if (id === "\0search-ui-entry") return entry;
  },
  configureServer(server) {
    server.middlewares.use(async (req, res, next) => {
      if (!req.url?.startsWith("/kb/")) return next();
      try {
        res.setHeader("content-type", "text/html");
        res.end(await server.transformIndexHtml(req.url,
          '<html><body><div id="root"></div><script type="module" src="/search-ui-entry.js"></script></body></html>'));
      } catch (error) {
        next(error);
      }
    });
  },
};


const hit = (name, n) => ({
  id: `${name}-${n}`, document_id: `doc-${n}`, seq: n-1,
  filename: `${name}.txt`, text: `${name} result ${n}`,
});
test("Search exposes result 11 and stops after the final server page", { timeout: 45_000 }, async (t) => {
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
  const page = await browser.newPage();
  page.setDefaultTimeout(8_000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const requests = [];
  let firstNextPage = true;
  let releaseSecondPage;
  const secondPageGate = new Promise((resolve) => { releaseSecondPage = resolve; });
  await page.route("**/api/**", async (route) => {
    const { pathname } = new URL(route.request().url());
    if (pathname === "/api/v1/auth/me") return route.fulfill({ json: { id: "user", is_admin: true } });
    if (pathname === "/api/v1/workspaces") return route.fulfill({ json: [{ id: "ws", name: "Workspace" }] });
    if (pathname === "/api/v1/workspaces/ws/kbs") return route.fulfill({ json: [
      { id: "one", name: "one", workspace_id: "ws", my_role: "owner" },
    ] });
    if (pathname === "/api/v1/kbs/one/readiness") return route.fulfill({ json: {} });
    if (pathname === "/api/v1/kbs/one/search") {
      const { q, offset = 0, top_k = 10 } = route.request().postDataJSON();
      requests.push([q, offset, top_k]);
      if (q === "alpha" && offset === 10 && firstNextPage) {
        firstNextPage = false;
        await secondPageGate;
        return route.fulfill({ status: 503, json: { message: "Search service unavailable" } });
      }
      const count = q === "alpha" ? 11 : 1;
      const all = Array.from({ length: count }, (_, i) => hit(q, i+1));
      return route.fulfill({ json: {
        results: all.slice(offset, offset + top_k), offset, top_k,
        has_more: offset + top_k < count, vector_used: false,
      } });
    }
    errors.push(`Unexpected API path ${pathname}`);
    return route.fulfill({ status: 500, json: { error: "Unexpected API path" } });
  });
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/kb/one/search?q=alpha`);
  const exact = (text) => page.getByText(text, { exact: true });
  await exact("alpha result 10").waitFor();
  assert.equal(await exact("alpha result 11").count(), 0);
  const previous = page.getByRole("button", { name: "Previous page", exact: true });
  const next = page.getByRole("button", { name: "Next page", exact: true });
  assert.equal(await next.count(), 1, "has_more=true must expose the next result page");
  const laterRequest = page.waitForRequest((req) => req.url().endsWith("/search") && req.postDataJSON()?.offset === 10);
  await next.click();
  try {
    await laterRequest;
    assert.equal(await next.isDisabled(), true, "an in-flight page cannot be skipped by another Next click");
    assert.equal(await previous.isEnabled(), true, "the user can return while a page is loading");
  } finally {
    releaseSecondPage();
  }
  await page.getByRole("button", { name: "Retry search", exact: true }).click();
  await exact("alpha result 11").waitFor();
  assert.equal(await next.isDisabled(), true, "has_more=false must stop at the last page");
  await previous.click();
  await exact("alpha result 10").waitFor();
  await next.click();
  await exact("alpha result 11").waitFor();
  await page.getByRole("textbox").fill("beta");
  await page.getByRole("button", { name: "Search", exact: true }).click();
  await exact("beta result 1").waitFor();
  assert.deepEqual(requests, [["alpha", 0, 10], ["alpha", 10, 10], ["alpha", 10, 10], ["beta", 0, 10]]);
  assert.deepEqual(errors, []);
});
