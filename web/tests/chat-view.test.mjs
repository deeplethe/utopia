import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { createServer } from "vite";
// Uses the optional browser setup from rss-default.test.mjs; no app dependency.
const require = createRequire(import.meta.url);
const { chromium } = require(
  process.env.CHAT_PLAYWRIGHT_PATH || "playwright-core",
);
const root = fileURLToPath(new URL("../", import.meta.url));
const entry = `
import React from 'react'; import {createRoot} from 'react-dom/client';
import {QueryClient,QueryClientProvider} from '@tanstack/react-query';
import {createRouter,createRootRoute,createRoute,RouterProvider,Outlet} from '@tanstack/react-router';
import {Chat} from '/src/pages/Chat.tsx'; import {liveAnswer} from '/src/liveAnswer.ts';
const parent=createRootRoute({component:Outlet});
const routes=['/kb/$kbId/chat','/kb/$kbId/chat/$conversationId'].map(path=>createRoute({getParentRoute:()=>parent,path,component:Chat}));
const router=createRouter({routeTree:parent.addChildren(routes)});
window.__go=to=>router.navigate({to}); window.__live=liveAnswer;
const client=new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
const tree=React.createElement(QueryClientProvider,{client},React.createElement(RouterProvider,{router}));
createRoot(document.getElementById('root')).render(location.search.includes('strict')?React.createElement(React.StrictMode,null,tree):tree);
`;
function plugin() {
  return {
    name: "chat-view-fixture",
    enforce: "pre",
    resolveId(id) {
      if (id === "/chat-view-entry.js") return "\0chat-view-entry";
    },
    load(id) {
      if (id === "\0chat-view-entry") return entry;
    },
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (!req.url.startsWith("/kb/")) return next();
        try {
          res.setHeader("content-type", "text/html");
          res.end(
            await server.transformIndexHtml(
              req.url,
              '<html><body><div id="root"></div><script type="module" src="/chat-view-entry.js"></script></body></html>',
            ),
          );
        } catch (e) {
          next(e);
        }
      });
    },
  };
}
const message = (content, role = "assistant") => ({
  role,
  content,
  steps: [],
  sources: [],
  created_at: "2026-01-01",
});
const deferred = () => {
  let resolve;
  const promise = new Promise((r) => (resolve = r));
  return { promise, resolve };
};

test(
  "real Chat view owns asynchronous work",
  { timeout: 120000 },
  async (t) => {
    const browser = await chromium.launch({
      headless: true,
      ...(process.env.CHAT_CHROMIUM_PATH
        ? { executablePath: process.env.CHAT_CHROMIUM_PATH }
        : {}),
    });
    t.after(() => browser.close());
    const server = await createServer({
      root,
      configFile: false,
      plugins: [plugin()],
      resolve: { alias: { "@": `${root}src` } },
      esbuild: { jsx: "automatic" },
      server: { host: "127.0.0.1", port: 0, hmr: false },
    });
    t.after(() => server.close());
    await server.listen();
    const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
    async function open(path, custom) {
      const context = await browser.newContext();
      const page = await context.newPage();
      page.setDefaultTimeout(10000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/api/**", async (route) => {
        const url = new URL(route.request().url());
        const p = url.pathname;
        if (await custom?.(route, p, url)) return;
        if (p === "/api/v1/auth/me")
          return route.fulfill({ json: { id: "user", is_admin: true } });
        if (p === "/api/v1/workspaces")
          return route.fulfill({ json: [{ id: "ws", name: "workspace" }] });
        if (p === "/api/v1/workspaces/ws/kbs")
          return route.fulfill({
            json: ["one", "two"].map((id) => ({
              id,
              name: id,
              workspace_id: "ws",
              my_role: "owner",
            })),
          });
        if (p.endsWith("/readiness"))
          return route.fulfill({ json: { has_chat_model: true } });
        if (p.endsWith("/conversations"))
          return route.fulfill({
            json: {
              conversations: ["a", "b"].map((id) => ({
                id,
                title: `Conversation ${id}`,
                created_at: "2026-01-01",
                updated_at: "2026-01-01",
              })),
              total: 2,
            },
          });
        if (p.endsWith("/stream"))
          return route.fulfill({
            contentType: "text/event-stream",
            body: "event: idle\ndata: {}\n\n",
          });
        if (/\/conversations\/[ab]$/.test(p))
          return route.fulfill({
            json: {
              messages: [
                message(`Answer ${p.endsWith("/a") ? "alpha" : "beta"}`),
              ],
            },
          });
        errors.push(`Unexpected ${p}`);
        await route.fulfill({
          status: 500,
          json: { error: "Unexpected request" },
        });
      });
      await page.goto(origin + path);
      await page.waitForFunction(() => !!window.__go);
      return { page, errors, close: () => context.close() };
    }
    for (const fail of [false, true])
      await t.test(
        `late A ${fail ? "failure" : "success"} cannot replace B`,
        async () => {
          const pending = deferred();
          const f = await open("/kb/one/chat/a", async (route, p) => {
            if (p.endsWith("/conversations/a")) {
              pending.resolve(route);
              return true;
            }
          });
          try {
            const a = await pending.promise;
            await f.page.evaluate(() => window.__go("/kb/one/chat/b"));
            await f.page.getByText("Answer beta", { exact: true }).waitFor();
            await a.fulfill(
              fail
                ? { status: 500, json: { error: "late failure" } }
                : { json: { messages: [message("Late alpha")] } },
            );
            await f.page.evaluate(() => new Promise(requestAnimationFrame));
            await f.page.evaluate(() => new Promise(requestAnimationFrame));
            assert.match(f.page.url(), /\/chat\/b$/);
            assert.equal(
              await f.page.getByText("Answer beta", { exact: true }).count(),
              1,
            );
            assert.equal(
              await f.page.getByText("Late alpha", { exact: true }).count(),
              0,
            );
            assert.deepEqual(f.errors, []);
          } finally {
            await f.close();
          }
        },
      );
    await t.test("A to B to A rejects the first A response", async () => {
      const pending = deferred();
      let count = 0;
      const f = await open("/kb/one/chat/a", async (route, p) => {
        if (p.endsWith("/conversations/a")) {
          if (++count === 1) {
            pending.resolve(route);
            return true;
          }
          await route.fulfill({
            json: { messages: [message("Newest alpha")] },
          });
          return true;
        }
      });
      try {
        const old = await pending.promise;
        await f.page.evaluate(() => window.__go("/kb/one/chat/b"));
        await f.page.getByText("Answer beta", { exact: true }).waitFor();
        await f.page.evaluate(() => window.__go("/kb/one/chat/a"));
        await f.page.getByText("Newest alpha", { exact: true }).waitFor();
        await old.fulfill({ json: { messages: [message("Obsolete alpha")] } });
        await f.page.evaluate(() => new Promise(requestAnimationFrame));
        await f.page.evaluate(() => new Promise(requestAnimationFrame));
        assert.equal(
          await f.page.getByText("Newest alpha", { exact: true }).count(),
          1,
        );
        assert.equal(
          await f.page.getByText("Obsolete alpha", { exact: true }).count(),
          0,
        );
      } finally {
        await f.close();
      }
    });
    await t.test(
      "late new conversation identity does not navigate away",
      async () => {
        const pending = deferred();
        const f = await open("/kb/one/chat", async (route, p) => {
          if (p === "/api/v1/kbs/one/chat") {
            pending.resolve(route);
            return true;
          }
        });
        try {
          await f.page.locator("textarea").fill("hello");
          await f.page.locator("textarea").press("Enter");
          const post = await pending.promise;
          await f.page.evaluate(() => window.__go("/kb/one/chat/b"));
          await f.page.getByText("Answer beta", { exact: true }).waitFor();
          await post.fulfill({
            contentType: "text/event-stream",
            body: 'event: conversation\ndata: {"id":"late"}\n\nevent: delta\ndata: {"text":"Background answer"}\n\nevent: done\ndata: {}\n\n',
          });
          await f.page.waitForFunction(
            () => window.__live.entry("one", "late")?.streaming === false,
          );
          assert.match(f.page.url(), /\/chat\/b$/);
          assert.equal(
            await f.page.evaluate(
              () => window.__live.entry("one", "late").turns.at(-1).content,
            ),
            "Background answer",
          );
        } finally {
          await f.close();
        }
      },
    );

    await t.test("new chat rejects a late detail response", async () => {
      const pending = deferred();
      const f = await open("/kb/one/chat/a", async (route, p) => {
        if (p.endsWith("/conversations/a")) {
          pending.resolve(route);
          return true;
        }
      });
      try {
        const old = await pending.promise;
        await f.page
          .getByRole("button", { name: "New chat", exact: true })
          .click();
        await old.fulfill({ json: { messages: [message("Late alpha")] } });
        await f.page.evaluate(() => new Promise(requestAnimationFrame));
        await f.page.evaluate(() => new Promise(requestAnimationFrame));
        assert.match(f.page.url(), /\/chat$/);
        assert.equal(
          await f.page.getByText("Late alpha", { exact: true }).count(),
          0,
        );
      } finally {
        await f.close();
      }
    });
    await t.test(
      "switching knowledge bases hides the previous transcript immediately",
      async () => {
        const pending = deferred();
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p === "/api/v1/kbs/two/conversations/b") {
            pending.resolve(route);
            return true;
          }
        });
        try {
          await f.page.getByText("Answer alpha", { exact: true }).waitFor();
          await f.page.evaluate(() => window.__go("/kb/two/chat/b"));
          const next = await pending.promise;
          assert.equal(
            await f.page.getByText("Answer alpha", { exact: true }).count(),
            0,
          );
          await next.fulfill({
            json: { messages: [message("Other library answer")] },
          });
          await f.page
            .getByText("Other library answer", { exact: true })
            .waitFor();
          assert.match(f.page.url(), /\/two\/chat\/b$/);
        } finally {
          await f.close();
        }
      },
    );
    await t.test(
      "current read failure stays on the conversation and can be retried",
      async () => {
        let count = 0;
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            await route.fulfill(
              ++count === 1
                ? { status: 500, json: { error: "temporary failure" } }
                : { json: { messages: [message("Recovered history")] } },
            );
            return true;
          }
        });
        try {
          await f.page.getByRole("alert").waitFor();
          assert.match(f.page.url(), /\/chat\/a$/);
          await f.page
            .getByRole("button", { name: "Retry", exact: true })
            .click();
          await f.page
            .getByText("Recovered history", { exact: true })
            .waitFor();
          assert.equal(count, 2);
        } finally {
          await f.close();
        }
      },
    );
    await t.test(
      "missing conversation keeps the existing new-chat redirect",
      async () => {
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            await route.fulfill({ status: 404, json: { error: "Not found" } });
            return true;
          }
        });
        try {
          await f.page.waitForURL("**/kb/one/chat");
          assert.equal(await f.page.getByRole("alert").count(), 0);
        } finally {
          await f.close();
        }
      },
    );
    await t.test(
      "StrictMode loads history and initializes one reattachment",
      async () => {
        let streams = 0;
        const f = await open("/kb/one/chat/a?strict", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            await route.fulfill({
              json: { messages: [message("Question", "user")] },
            });
            return true;
          }
          if (p.endsWith("/stream")) {
            streams++;
            await route.fulfill({
              contentType: "text/event-stream",
              body: 'event: snapshot\ndata: {"content":"Restored answer","steps":[],"sources":[]}\n\nevent: done\ndata: {}\n\n',
            });
            return true;
          }
        });
        try {
          await f.page.getByText("Restored answer", { exact: true }).waitFor();
          assert.equal(streams, 1);
          assert.deepEqual(f.errors, []);
        } finally {
          await f.close();
        }
      },
    );
    await t.test(
      "late reattachment snapshot cannot create a stale live entry",
      async () => {
        const pending = deferred();
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            await route.fulfill({
              json: { messages: [message("Question", "user")] },
            });
            return true;
          }
          if (p.endsWith("/conversations/a/stream")) {
            pending.resolve(route);
            return true;
          }
        });
        try {
          const old = await pending.promise;
          await f.page.evaluate(() => window.__go("/kb/one/chat/b"));
          await f.page.getByText("Answer beta", { exact: true }).waitFor();
          await old.fulfill({
            contentType: "text/event-stream",
            body: 'event: snapshot\ndata: {"content":"Late snapshot","steps":[],"sources":[]}\n\nevent: done\ndata: {}\n\n',
          });
          await f.page.evaluate(() => new Promise(requestAnimationFrame));
          await f.page.evaluate(() => new Promise(requestAnimationFrame));
          assert.equal(
            await f.page.evaluate(() => window.__live.entry("one", "a")),
            null,
          );
          assert.equal(
            await f.page.getByText("Answer beta", { exact: true }).count(),
            1,
          );
        } finally {
          await f.close();
        }
      },
    );

    await t.test(
      "selecting the current conversation does not invalidate its pending read",
      async () => {
        const pending = deferred();
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            pending.resolve(route);
            return true;
          }
        });
        try {
          const a = await pending.promise;
          await f.page.getByText("Conversation a", { exact: true }).click();
          await a.fulfill({ json: { messages: [message("Current answer")] } });
          await f.page.getByText("Current answer", { exact: true }).waitFor();
        } finally {
          await f.close();
        }
      },
    );
  },
);
