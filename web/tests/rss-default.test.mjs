import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { createServer } from "vite";

// From web/ (no changes to app dependencies or lockfile):
// npm install --prefix /tmp/utopia-rss-browser-test --no-audit --no-fund --package-lock=false playwright-core@1.58.2
// RSS_PLAYWRIGHT_PATH=/tmp/utopia-rss-browser-test/node_modules/playwright-core RSS_CHROMIUM_PATH=/home/jik/.cache/ms-playwright/chromium_headless_shell-1223/chrome-headless-shell-linux64/chrome-headless-shell node --test tests/rss-default.test.mjs
// Point RSS_CHROMIUM_PATH at an installed Chromium on other machines.
// Regression probes: prefix the test command with RSS_FORM_MUTATION=wrong-default
// or RSS_FORM_MUTATION=disconnected-payload; each MUST fail. Mutations exist only
// in Vite's in-memory module transform: production files are never rewritten.
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.RSS_PLAYWRIGHT_PATH || "playwright-core");
const root = fileURLToPath(new URL("../", import.meta.url));
const mutation = process.env.RSS_FORM_MUTATION;
assert.ok(!mutation || ["wrong-default", "disconnected-payload"].includes(mutation));

const entry = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { SourceModal, SourceBar } from '/src/pages/Library.tsx';
const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
const source = {
  id: 'rss-bar-source', kind: 'rss', name: 'RSS bar test',
  config: { feed_url: 'https://example.com/feed.xml', content_mode: 'full_new_items' },
  last_sync_status: 'never', last_sync_at: null, last_sync_error: null,
  sync_interval_minutes: null, sync_cron: null, missing_count: 0,
  rss_full_content_pending_count: 1, rss_full_content_queued_count: 2,
  rss_full_content_retrying_count: 3, rss_full_content_complete_count: 4,
  rss_full_content_terminal_count: 5,
};
createRoot(document.getElementById('bar')).render(
  React.createElement(SourceBar, { kbId: 'rss-form-test', source,
    syncing: false, historyOpen: false, onToggleHistory() {}, onSync() {},
    onEdit() {}, onCleanup() {}, onToken() {} }));
createRoot(document.getElementById('root')).render(
  React.createElement(QueryClientProvider, { client },
    React.createElement(SourceModal, { kbId: 'rss-form-test', onDone: (id, isApi) => {
      document.getElementById('done').textContent = JSON.stringify({ id, isApi });
    } })));
`;

// Only expose the real component to this isolated fixture. Its state, DOM,
// event handlers, React Query mutation and api.createSource all run unchanged.
function fixturePlugin() {
  return {
    name: "rss-modal-browser-fixture",
    enforce: "pre",
    resolveId(id) {
      if (id === "/rss-test-entry.js") return "\0rss-test-entry.js";
    },
    load(id) {
      if (id === "\0rss-test-entry.js") return entry;
    },
    transform(code, id) {
      if (id.split("?")[0] !== `${root}src/pages/Library.tsx`) return;
      if (mutation) {
        const [before, after] = mutation === "wrong-default"
          ? ['useState<RssContentMode>("feed")', 'useState<RssContentMode>("full_new_items")']
          : ['content_mode: rssContentMode', 'content_mode: "feed"'];
        assert.ok(code.includes(before), `Regression probe target missing: ${before}`);
        code = code.replace(before, after);
      }
      return `${code}\nexport { SourceModal, SourceBar };\n`;
    },
    configureServer(server) {
      server.middlewares.use("/rss-test", async (_req, res, next) => {
        try {
          const html = await server.transformIndexHtml("/rss-test", `<!doctype html>
<html><head><title>RSS source form test</title></head><body>
<div id="bar"></div><div id="root"></div><output id="done"></output>
<script type="module" src="/rss-test-entry.js"></script></body></html>`);
          res.setHeader("Content-Type", "text/html");
          res.end(html);
        } catch (error) { next(error); }
      });
    },
  };
}

// No router harness exists in this crate; guard the production registration and
// handler source directly, without constructing a parallel test-only router.
test("entry diagnostics has no public route or client", async () => {
  for (const path of ["../crates/utopia-server/src/api/mod.rs", "../crates/utopia-server/src/api/sources_routes.rs", "src/api.ts"]) {
    const source = await readFile(new URL(path, new URL("../", import.meta.url)), "utf8");
    assert.doesNotMatch(source, /rss-full-content\/entries|full_content_entries|rssFullContentEntries|RssFullContentEntryDiagnostic/, path);
  }
});

test("production SourceModal RSS submissions", { timeout: 90_000 }, async (t) => {
  // Fail before starting Vite if the caller has no usable browser installed.
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.RSS_CHROMIUM_PATH ? { executablePath: process.env.RSS_CHROMIUM_PATH } : {}),
  });
  t.after(() => browser.close());
  const server = await createServer({
    root,
    configFile: false,
    plugins: [fixturePlugin()],
    esbuild: { jsx: "automatic" },
    server: { host: "127.0.0.1", port: 0, hmr: false },
  });
  t.after(() => server.close());
  await server.listen();
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
  // Verify fixture readiness before browser navigation.
  assert.equal((await fetch(`${origin}/rss-test`)).status, 200);

  await t.test("SourceBar keeps labeled counts without raw entry diagnostics", async () => {
    const page = await browser.newPage();
    try {
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto(`${origin}/rss-test`);
      await page.locator("#bar").getByText("Full articles", { exact: true }).waitFor();
      assert.ok(await page.locator("#bar").getByText(
        "pending 1 · queued 2 · retrying 3 · complete 4 · terminal 5", { exact: true }).isVisible());
      assert.equal(await page.locator('#bar a[href*="rss-full-content/entries"]').count(), 0);
      assert.equal(await page.locator("#bar").getByText("Entry diagnostics", { exact: true }).count(), 0);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  for (const mode of ["feed", "full_new_items"]) {
    await t.test(mode === "feed" ? "untouched default submits feed" : "explicit full_new_items reaches createSource", async () => {
      const context = await browser.newContext({ viewport: { width: 1280, height: 1000 } });
      try {
        const page = await context.newPage();
        page.setDefaultTimeout(15_000);
        const errors = [];
        const submissions = [];
        page.on("pageerror", (error) => errors.push(error.message));
        page.on("console", (message) => { if (message.type() === "error") errors.push(message.text()); });
        // No backend or external network: only fixture assets are allowed.
        // Keep the production API client, intercepting at its HTTP boundary.
        await page.route("**/*", async (route) => {
          const request = route.request();
          if (request.url() === `${origin}/api/v1/kbs/rss-form-test/sources` && request.method() === "POST") {
            submissions.push(request.postDataJSON());
            return route.fulfill({ json: { source: { id: "created-rss-source" } } });
          }
          if (request.url().startsWith(`${origin}/`) && !new URL(request.url()).pathname.startsWith("/api/")) return route.continue();
          errors.push(`Unexpected request: ${request.method()} ${request.url()}`);
          return route.abort();
        });
        await page.goto(`${origin}/rss-test`);
        // Upstream source kinds now live in the real modal's dropdown.
        const dialog = page.getByRole("dialog");
        await dialog.getByRole("button", { name: "Folder", exact: true }).click();
        await dialog.getByRole("button", { name: "RSS feed", exact: true }).click();
        const modeSelect = page.locator("select").filter({ has: page.locator('option[value="full_new_items"]') });
        assert.equal(await modeSelect.count(), 1);
        assert.equal(await modeSelect.inputValue(), "feed", "RSS must initially display feed-only");
        if (mode === "full_new_items") await modeSelect.selectOption("full_new_items");
        assert.equal(await modeSelect.inputValue(), mode);
        const inputs = page.locator('input:not([type="checkbox"]):not([type="radio"])');
        await inputs.first().fill("  RSS browser test  ");
        await inputs.nth(1).fill("  https://example.com/feed.xml  ");
        await page.getByRole("button", { name: "Create", exact: true }).click();
        await page.waitForFunction(() => document.getElementById("done").textContent !== "");
        assert.deepEqual(submissions, [{
          kind: "rss",
          name: "RSS browser test",
          config: { feed_url: "https://example.com/feed.xml", content_mode: mode },
          icon: null,
          sync_interval_minutes: null,
          sync_cron: null,
        }], "The selected mode must survive the production createSource POST");
        assert.deepEqual(JSON.parse(await page.locator("#done").textContent()), { id: "created-rss-source", isApi: false });
        assert.deepEqual(errors, [], "No runtime, console, or unexpected network errors");
      } finally { await context.close(); }
    });
  }
});
