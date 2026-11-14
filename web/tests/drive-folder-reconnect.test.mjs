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
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { GoogleDrivePanel } from '/src/pages/GoogleDrivePanel.tsx';
import { api } from '/src/api.ts';
const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
window.invalidateSource = () => client.invalidateQueries({ queryKey: ['sources', 'one'] });
function Fixture() {
  const {data} = useQuery({queryKey: ['sources', 'one'], queryFn: () => api.sources('one')});
  const source = data?.sources[0];
  if (!source) return null;
  return React.createElement(React.Fragment, null,
    React.createElement('output', null, source.config.marker),
    React.createElement(GoogleDrivePanel, { kbId: 'one', source }));
}
createRoot(document.getElementById('root')).render(
  React.createElement(QueryClientProvider, { client }, React.createElement(Fixture)),
);
`;
const fixture = {
  name: "drive-dialog-fixture",
  enforce: "pre",
  resolveId(id) {
    if (id === "/drive-dialog-entry.js") return "\0drive-dialog-entry";
  },
  load(id) {
    if (id === "\0drive-dialog-entry") return entry;
  },
  configureServer(server) {
    server.middlewares.use(async (req, res, next) => {
      if (!req.url?.startsWith("/kb/")) return next();
      try {
        res.setHeader("content-type", "text/html");
        res.end(await server.transformIndexHtml(req.url,
          '<html><body><div id="root"></div><script type="module" src="/drive-dialog-entry.js"></script></body></html>'));
      } catch (error) {
        next(error);
      }
    });
  },
};
const oldFolder = { id: "old", name: "Old folder" };
const draftFolder = { id: "draft", name: "Draft folder" };
const newFolder = { id: "new", name: "New folder" };
const previous = { connected_at: "2026-11-01", connected_by: "user", account_email: "first@example.com",
  folders: [oldFolder], marker: "initial source" };
const updated = (overrides) => ({ ...previous, connected_at: "2026-11-14", ...overrides });
const routeBase = "/api/v1/kbs/one/sources/drive/google-drive";

test("Google Drive folder drafts follow the connected account", { timeout: 45_000 }, async (t) => {
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
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
  async function setup() {
    const page = await browser.newPage();
    page.setDefaultTimeout(8_000);
    const state = { config: previous, saved: [], folderParents: [], errors: [] };
    page.on("pageerror", (error) => state.errors.push(error.message));
    await page.addInitScript(() => {
      window.popupCloses = 0;
      window.open = () => ({ closed: false, location: {}, close() { this.closed = true; window.popupCloses++; } });
    });
    await page.route("**/api/**", async (route) => {
      const { pathname, searchParams } = new URL(route.request().url());
      const method = route.request().method();
      if (pathname === "/api/v1/auth/me") return route.fulfill({ json: { id: "user", is_admin: true } });
      if (pathname === "/api/v1/google-drive/status") return route.fulfill({ json: { enabled: true } });
      if (pathname === "/api/v1/kbs/one/sources") return route.fulfill({ json: { sources: [{ id: "drive", config: state.config }] } });
      if (pathname === routeBase + "/connect") return route.fulfill({ json: { url: origin + "/fake-google-drive" } });
      if (pathname === routeBase + "/connection" && method === "DELETE") {
        state.config = { marker: "disconnected source" };
        return route.fulfill({ json: { ok: true } });
      }
      if (pathname === routeBase + "/folders" && method === "PUT") {
        const { folders } = route.request().postDataJSON();
        state.saved.push(folders);
        state.config = { ...state.config, folders };
        return route.fulfill({ json: { source: { id: "drive", config: state.config } } });
      }
      if (pathname === routeBase + "/folders") {
        const parent = searchParams.get("parent");
        state.folderParents.push([state.config.account_email, parent]);
        const folders = state.config.account_email === "second@example.com" ? (parent ? [] : [newFolder]) :
          (parent ? [] : [draftFolder]);
        return route.fulfill({ json: { folders } });
      }
      state.errors.push(`Unexpected API path ${pathname}`);
      return route.fulfill({ status: 500, json: { error: "Unexpected API path" } });
    });
    await page.goto(origin + "/kb/one/library");
    const exact = (text) => page.getByText(text, { exact: true });
    const save = page.getByRole("button", { name: "Save folders and sync", exact: true });
    const startup = page.getByRole("button", { name: "Reconnect", exact: true });
    await exact("Old folder").waitFor();
    const chooseDraft = () => page.getByRole("checkbox", { name: "Draft folder", exact: true }).click();
    async function reconnect(next, connect = startup) {
      await connect.click();
      await exact("Finish in the Google window…").waitFor();
      state.config = next;
      await exact(next.marker).waitFor();
      await exact(`Connected as ${next.account_email}`).waitFor();
      await page.waitForFunction(() => window.popupCloses === 1);
    }
    return { page, exact, save, state, chooseDraft, reconnect };
  }

  await t.test("same-connection refresh and same verified account reconnect retain the unsaved draft", async () => {
    const f = await setup();
    await f.chooseDraft();
    assert.equal(await f.save.isEnabled(), true);
    f.state.config = { ...previous, marker: "ordinary refresh" };
    await f.page.evaluate(() => window.invalidateSource());
    await f.exact("ordinary refresh").waitFor();
    assert.equal(await f.save.isEnabled(), true);
    await f.page.getByRole("button", { name: "Open", exact: true }).click();
    await f.exact("No folders here.").waitFor();
    await f.reconnect(updated({ account_email: "FIRST@EXAMPLE.COM", marker: "same grant refreshed" }));
    assert.deepEqual(f.state.folderParents.filter(([email]) => email === "FIRST@EXAMPLE.COM"), [["FIRST@EXAMPLE.COM", null]]);
    const wrote = f.page.waitForResponse((r) => r.request().method() === "PUT" && r.url().endsWith("/folders"));
    await f.save.click();
    await wrote;
    assert.deepEqual(f.state.saved, [[oldFolder, draftFolder]]);
    assert.deepEqual(f.state.errors, []);
    await f.page.close();
  });
  await t.test("another account uses its loaded configuration and never browses the old breadcrumb", async () => {
    const f = await setup();
    await f.chooseDraft();
    await f.page.getByRole("button", { name: "Open", exact: true }).click();
    await f.exact("No folders here.").waitFor();
    await f.reconnect(updated({ account_email: "second@example.com", folders: [], marker: "new account" }));
    assert.equal(await f.exact("Old folder").count(), 0, "the other account must not inherit the old selection");
    await f.exact("No folders chosen yet.").waitFor();
    assert.equal(await f.save.isDisabled(), true);
    assert.deepEqual(f.state.folderParents.filter(([email]) => email === "second@example.com"), [["second@example.com", null]]);
    f.state.config = { ...f.state.config, folders: [newFolder], marker: "new account with saved folder" };
    await f.page.evaluate(() => window.invalidateSource());
    await f.exact("new account with saved folder").waitFor();
    // Folder listing and the hydrated chosen-folder chip both carry the name.
    assert.equal(await f.exact("New folder").count(), 2);
    assert.equal(await f.save.isDisabled(), true);
    assert.deepEqual(f.state.saved, []);
    await f.page.getByRole("checkbox", { name: "New folder", exact: true }).click();
    const wrote = f.page.waitForResponse((r) => r.request().method() === "PUT" && r.url().endsWith("/folders"));
    await f.save.click();
    await wrote;
    assert.deepEqual(f.state.saved, [[]], "clearing the current account's folder must submit an empty selection");
    assert.deepEqual(f.state.errors, []);
    await f.page.close();
  });
  await t.test("disconnecting and later reconnecting the same account starts from scratch", async () => {
    const f = await setup();
    await f.chooseDraft();
    await f.page.getByRole("button", { name: "Disconnect", exact: true }).click();
    await f.page.getByRole("dialog").getByRole("button", { name: "Disconnect", exact: true }).click();
    await f.exact("disconnected source").waitFor();
    assert.equal(await f.exact("Old folder").count(), 0);
    await f.reconnect(updated({ folders: [], marker: "new connection after disconnect" }),
      f.page.getByRole("button", { name: "Connect Google Drive", exact: true }));
    assert.equal(await f.exact("Old folder").count(), 0, "a new grant after disconnect must start clean");
    await f.exact("No folders chosen yet.").waitFor();
    assert.equal(await f.save.isDisabled(), true);
    assert.deepEqual(f.state.errors, []);
    await f.page.close();
  });
});
