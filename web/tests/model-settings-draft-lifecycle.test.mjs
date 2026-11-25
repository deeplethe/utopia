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
import { Settings } from '/src/pages/Settings.tsx';
const parent = createRootRoute({ component: Outlet });
const account = createRoute({ getParentRoute: () => parent, id: 'account', component: Outlet });
const admin = createRoute({ getParentRoute: () => account, path: '/admin',
  validateSearch: ({tab}) => ({tab}), component: Settings });
const router = createRouter({ routeTree: parent.addChildren([account.addChildren([admin])]) });
const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
createRoot(document.getElementById('root')).render(
  React.createElement(QueryClientProvider, { client }, React.createElement(RouterProvider, { router })),
);
`;
const fixture = {
  name: "models-ui-fixture",
  enforce: "pre",
  resolveId(id) {
    if (id === "/models-ui-entry.js") return "\0models-ui-entry";
  },
  load(id) {
    if (id === "\0models-ui-entry") return entry;
  },
  configureServer(server) {
    server.middlewares.use(async (req, res, next) => {
      if (!req.url?.startsWith("/admin")) return next();
      try {
        res.setHeader("content-type", "text/html");
        res.end(await server.transformIndexHtml(req.url,
          '<html><body><div id="root"></div><script type="module" src="/models-ui-entry.js"></script></body></html>'));
      } catch (error) {
        next(error);
      }
    });
  },
};


const initial = {
  chat_base_url: "https://chat.example.test/v1", chat_model: "saved-chat", chat_reasoning_effort: "medium", has_chat_key: true,
  embed_base_url: "https://embed.example.test/v1", embed_model: "saved-embed", has_embed_key: true,
};

function deferred() {
  let resolve;
  const promise = new Promise((finish) => { resolve = finish; });
  return { promise, resolve };
}

test("model saves preserve drafts and reconcile only the saved revision", { timeout: 45_000 }, async (t) => {
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

  async function open(t) {
    const page = await browser.newPage();
    t.after(() => page.close());
    page.setDefaultTimeout(8_000);
    const requests = [], errors = [], unexpected = [];
    let saved = { ...initial };
    let putGate;
    let failNextReadback = false;
    const holdNextPut = () => {
      assert.equal(putGate, undefined);
      putGate = { entered: deferred(), release: deferred() };
      return putGate;
    };
    page.on("pageerror", (error) => errors.push(error.message));
    await page.route("**/api/**", async (route) => {
      const { pathname } = new URL(route.request().url());
      if (pathname === "/api/v1/workspaces") return route.fulfill({ json: [{ id: "ws", name: "Workspace" }] });
      if (pathname === "/api/v1/workspaces/ws/kbs") return route.fulfill({ json: [
        { id: "one", name: "one", workspace_id: "ws", my_role: "owner" },
      ] });
      if (pathname === "/api/v1/workspaces/ws/settings") {
        if (route.request().method() !== "PUT") {
          if (failNextReadback) {
            failNextReadback = false;
            return route.fulfill({ status: 503, json: { message: "Could not reload settings" } });
          }
          return route.fulfill({ json: saved });
        }
        const body = route.request().postDataJSON();
        requests.push(body);
        if (putGate) {
          const gate = putGate;
          putGate = undefined;
          gate.entered.resolve();
          await gate.release.promise;
        }
        // Match the real transport: PUT returns only {ok}, canonical values come
        // back on GET, and no API read contains the contents of a key.
        for (const field of ["chat_base_url", "chat_model", "embed_base_url", "embed_model"]) {
          saved[field] = body[field]?.trim() || null;
        }
        if (body.chat_reasoning_effort !== undefined) {
          saved.chat_reasoning_effort = body.chat_reasoning_effort.trim() || null;
        }
        return route.fulfill({ json: { ok: true } });
      }
      unexpected.push(pathname);
      return route.fulfill({ status: 500, json: { error: "Unexpected API path" } });
    });
    await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/admin?tab=models`);
    const chat = page.locator("section").filter({ has: page.getByRole("heading", { name: "Chat model", exact: true }) });
    const embed = page.locator("section").filter({ has: page.getByRole("heading", { name: /^Embedding model/ }) });
    const model = (card) => card.getByPlaceholder(card === chat ? "deepseek-chat" : "bge-m3");
    const key = (card) => card.locator('input[type="password"]');
    const submit = (card) => card.getByRole("button", { name: "Save", exact: true });
    const probe = (card) => card.getByRole("button", { name: "Test connection", exact: true });
    const waitValue = (card, value) => page.waitForFunction(
      ({ placeholder, value }) => document.querySelector(`input[placeholder="${placeholder}"]`)?.value === value,
      { placeholder: card === chat ? "deepseek-chat" : "bge-m3", value },
    );
    const nextSettings = () => page.waitForResponse((response) =>
      response.request().method() === "GET" && response.url().endsWith("/settings"));
    await waitValue(chat, initial.chat_model);
    await waitValue(embed, initial.embed_model);
    assert.equal(await key(chat).inputValue(), "");
    assert.equal(await key(embed).inputValue(), "");
    return {
      page, requests, errors, unexpected, holdNextPut, chat, embed, model,
      key, submit, probe, waitValue, nextSettings, failReadback: () => { failNextReadback = true; },
    };
  }

  function expectEmbedPayload(body, embedModel) {
    assert.equal(body.chat_base_url, initial.chat_base_url);
    assert.equal(body.chat_model, initial.chat_model);
    assert.equal(body.embed_base_url, initial.embed_base_url);
    assert.equal(body.embed_model.trim(), embedModel);
    assert.equal(body.embed_api_key ?? "", "");
    assert.equal(body.chat_api_key, undefined);
    assert.equal(body.chat_reasoning_effort, undefined);
  }

  async function editChat(f) {
    await f.model(f.chat).fill("draft-chat");
    await f.key(f.chat).fill("fake-local-chat-key");
  }

  async function assertSibling(f) {
    assert.equal(await f.model(f.chat).inputValue(), "draft-chat");
    assert.equal(await f.key(f.chat).inputValue(), "fake-local-chat-key");
    assert.equal(await f.probe(f.chat).isDisabled(), true);
    assert.deepEqual(f.errors, []);
    assert.deepEqual(f.unexpected, []);
  }

  await t.test("saving Embed does not submit or overwrite Chat drafts", async (t) => {
    const f = await open(t);
    await editChat(f);
    await f.model(f.embed).fill("  new-embed  ");
    const refreshed = f.nextSettings();
    await f.submit(f.embed).click();
    await refreshed;
    await f.waitValue(f.embed, "new-embed");
    await f.embed.getByText("Saved", { exact: true }).waitFor();
    assert.equal(f.requests.length, 1);
    expectEmbedPayload(f.requests[0], "new-embed");
    assert.equal(await f.probe(f.embed).isEnabled(), true);
    await assertSibling(f);
  });

  await t.test("a normalized no-op reads back the unchanged server value", async (t) => {
    const f = await open(t);
    await editChat(f);
    await f.model(f.embed).fill("  saved-embed  ");
    assert.equal(await f.probe(f.embed).isDisabled(), true);
    const refreshed = f.nextSettings();
    await f.submit(f.embed).click();
    await refreshed;
    await f.waitValue(f.embed, "saved-embed");
    await f.embed.getByText("Saved", { exact: true }).waitFor();
    assert.equal(f.requests.length, 1);
    expectEmbedPayload(f.requests[0], "saved-embed");
    assert.equal(await f.probe(f.embed).isEnabled(), true);
    await assertSibling(f);
  });

  await t.test("a saved older revision cannot wipe a model name and key edited during the PUT", async (t) => {
    const f = await open(t);
    await editChat(f);
    await f.model(f.embed).fill("  submitted-embed  ");
    await f.key(f.embed).fill("fake-first-embed-key");
    const gate = f.holdNextPut();
    const refreshed = f.nextSettings();
    try {
      await f.submit(f.embed).click();
      await gate.entered.promise;
      assert.equal(f.requests.length, 1);
      assert.equal(f.requests[0].embed_model.trim(), "submitted-embed");
      assert.equal(f.requests[0].embed_api_key, "fake-first-embed-key");
      assert.equal(f.requests[0].chat_model, initial.chat_model);
      assert.equal(f.requests[0].chat_api_key, undefined);
      await f.model(f.embed).fill("  later-embed  ");
      await f.key(f.embed).fill("fake-newer-embed-key");
    } finally {
      gate.release.resolve();
    }
    await refreshed;
    await f.submit(f.embed).waitFor();
    assert.equal(await f.model(f.embed).inputValue(), "  later-embed  ");
    assert.equal(await f.key(f.embed).inputValue(), "fake-newer-embed-key");
    assert.equal(await f.probe(f.embed).isDisabled(), true);
    await f.embed.getByText("Unsaved changes. Save this card to test them.", { exact: true }).waitFor();
    assert.equal(await f.embed.getByText("Saved", { exact: true }).count(), 0);
    await assertSibling(f);
    const savedLatest = f.nextSettings();
    await f.submit(f.embed).click();
    await savedLatest;
    await f.waitValue(f.embed, "later-embed");
    assert.equal(f.requests.length, 2);
    assert.equal(f.requests[1].embed_model.trim(), "later-embed");
    assert.equal(f.requests[1].embed_api_key, "fake-newer-embed-key");
    assert.equal(f.requests[1].chat_api_key, undefined);
    assert.equal(await f.probe(f.embed).isEnabled(), true);
    await assertSibling(f);
  });

  await t.test("reload after a failed readback confirms the original save without another PUT", async (t) => {
    const f = await open(t);
    await editChat(f);
    await f.model(f.embed).fill("  new-embed  ");
    f.failReadback();
    await f.submit(f.embed).click();
    const reload = f.page.getByRole("button", { name: "Reload settings", exact: true });
    await reload.waitFor();
    assert.equal(await f.model(f.embed).inputValue(), "  new-embed  ");
    await reload.click();
    await f.waitValue(f.embed, "new-embed");
    await f.embed.getByText("Saved", { exact: true }).waitFor();
    assert.equal(f.requests.length, 1);
    await assertSibling(f);
  });
});
