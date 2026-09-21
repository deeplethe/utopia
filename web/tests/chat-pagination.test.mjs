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
window.__invalidate=()=>client.invalidateQueries({queryKey:["conversations"]});
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

test("real Chat conversation pagination", { timeout: 120000 }, async (t) => {
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

  await t.test(
    "all 65 conversations are reachable through bounded pages",
    async () => {
      const offsets = [];
      const rows = Array.from({ length: 65 }, (_, i) => ({
        id: `c${i}`,
        title: `Conversation ${i}`,
        created_at: "2026-01-01",
        updated_at: "2026-01-01",
      }));
      const f = await open("/kb/one/chat", async (route, p, url) => {
        if (p.endsWith("/conversations")) {
          const offset = Number(url.searchParams.get("offset"));
          const limit = Number(url.searchParams.get("limit"));
          offsets.push(offset);
          assert.equal(limit, 30);
          await route.fulfill({
            json: {
              conversations: rows.slice(offset, offset + limit),
              total: rows.length,
            },
          });
          return true;
        }
      });
      try {
        await f.page.getByText("Conversation 29", { exact: true }).waitFor();
        await f.page
          .getByRole("button", {
            name: "Load earlier conversations",
            exact: true,
          })
          .click();
        await f.page.getByText("Conversation 59", { exact: true }).waitFor();
        await f.page
          .getByRole("button", {
            name: "Load earlier conversations",
            exact: true,
          })
          .click();
        await f.page.getByText("Conversation 64", { exact: true }).waitFor();
        assert.equal(
          await f.page
            .locator("aside")
            .getByText(/^Conversation \d+$/)
            .count(),
          65,
        );
        assert.equal(
          await f.page
            .getByRole("button", {
              name: "Load earlier conversations",
              exact: true,
            })
            .count(),
          0,
        );
        assert.deepEqual(offsets, [0, 30, 60]);
      } finally {
        await f.close();
      }
    },
  );

  const rows = (n) =>
    Array.from({ length: n }, (_, i) => ({
      id: `c${i}`,
      title: `Conversation ${i}`,
      created_at: "2026-01-01",
      updated_at: "2026-01-01",
    }));
  for (const total of [0, 30, 31, 60])
    await t.test(`page boundary ${total}`, async () => {
      const f = await open("/kb/one/chat", async (route, p, url) => {
        if (p.endsWith("/conversations")) {
          const offset = Number(url.searchParams.get("offset"));
          await route.fulfill({
            json: {
              conversations: rows(total).slice(offset, offset + 30),
              total,
            },
          });
          return true;
        }
      });
      try {
        if (total === 0)
          await f.page
            .getByText("No conversations yet.", { exact: true })
            .waitFor();
        else
          await f.page
            .getByText(`Conversation ${Math.min(total, 30) - 1}`, {
              exact: true,
            })
            .waitFor();
        if (total > 30) {
          await f.page
            .getByRole("button", {
              name: "Load earlier conversations",
              exact: true,
            })
            .click();
          await f.page
            .getByText(`Conversation ${total - 1}`, { exact: true })
            .waitFor();
        }
        assert.equal(
          await f.page
            .locator("aside")
            .getByText(/^Conversation \d+$/)
            .count(),
          total,
        );
        assert.equal(
          await f.page
            .getByRole("button", {
              name: "Load earlier conversations",
              exact: true,
            })
            .count(),
          0,
        );
      } finally {
        await f.close();
      }
    });
  await t.test(
    "failed next page retains loaded rows and retries the same offset",
    async () => {
      let attempts = 0;
      const f = await open("/kb/one/chat", async (route, p, url) => {
        if (p.endsWith("/conversations")) {
          const offset = Number(url.searchParams.get("offset"));
          if (offset === 30 && ++attempts === 1)
            await route.fulfill({
              status: 500,
              json: { error: "page failed" },
            });
          else
            await route.fulfill({
              json: {
                conversations: rows(35).slice(offset, offset + 30),
                total: 35,
              },
            });
          return true;
        }
      });
      try {
        await f.page.getByText("Conversation 29", { exact: true }).waitFor();
        await f.page
          .getByRole("button", {
            name: "Load earlier conversations",
            exact: true,
          })
          .click();
        await f.page.getByRole("alert").waitFor();
        assert.equal(
          await f.page.getByText("Conversation 29", { exact: true }).count(),
          1,
        );
        await f.page
          .getByRole("button", { name: "Retry", exact: true })
          .click();
        await f.page.getByText("Conversation 34", { exact: true }).waitFor();
        assert.equal(attempts, 2);
      } finally {
        await f.close();
      }
    },
  );
  await t.test("a late page cannot enter a changed search", async () => {
    const pending = deferred();
    const f = await open("/kb/one/chat", async (route, p, url) => {
      if (p.endsWith("/conversations")) {
        if (url.searchParams.get("q"))
          await route.fulfill({
            json: {
              conversations: [
                { ...rows(1)[0], id: "filtered", title: "Filtered result" },
              ],
              total: 1,
            },
          });
        else if (url.searchParams.get("offset") === "30")
          pending.resolve(route);
        else
          await route.fulfill({ json: { conversations: rows(30), total: 65 } });
        return true;
      }
    });
    try {
      await f.page.getByText("Conversation 29", { exact: true }).waitFor();
      await f.page
        .getByRole("button", {
          name: "Load earlier conversations",
          exact: true,
        })
        .click();
      const old = await pending.promise;
      await f.page.getByPlaceholder("Search chats").fill("filtered");
      await f.page.getByText("Filtered result", { exact: true }).waitFor();
      await old.fulfill({
        json: { conversations: rows(65).slice(30, 60), total: 65 },
      });
      await f.page.evaluate(() => new Promise(requestAnimationFrame));
      assert.equal(
        await f.page.getByText("Conversation 30", { exact: true }).count(),
        0,
      );
      assert.equal(
        await f.page.getByText("Filtered result", { exact: true }).count(),
        1,
      );
    } finally {
      await f.close();
    }
  });
  await t.test(
    "overlapping pages deduplicate by ID but retain identical titles",
    async () => {
      const list = rows(35);
      list[0].title = list[1].title = "Same title";
      const f = await open("/kb/one/chat", async (route, p, url) => {
        if (p.endsWith("/conversations")) {
          const second = url.searchParams.get("offset") === "30";
          await route.fulfill({
            json: {
              conversations: second ? list.slice(29) : list.slice(0, 30),
              total: 35,
            },
          });
          return true;
        }
      });
      try {
        await f.page.getByText("Conversation 29", { exact: true }).waitFor();
        await f.page
          .getByRole("button", {
            name: "Load earlier conversations",
            exact: true,
          })
          .click();
        await f.page.getByText("Conversation 34", { exact: true }).waitFor();
        assert.equal(
          await f.page.getByText("Conversation 29", { exact: true }).count(),
          1,
        );
        assert.equal(
          await f.page.getByText("Same title", { exact: true }).count(),
          2,
        );
      } finally {
        await f.close();
      }
    },
  );
  await t.test("search results continue within their filter", async () => {
    const offsets = [];
    const f = await open("/kb/one/chat", async (route, p, url) => {
      if (!p.endsWith("/conversations")) return;
      const offset = Number(url.searchParams.get("offset"));
      const q = url.searchParams.get("q");
      if (q) offsets.push([q, offset]);
      await route.fulfill({
        json: {
          conversations: rows(q ? 35 : 1).slice(offset, offset + 30),
          total: q ? 35 : 1,
        },
      });
      return true;
    });
    try {
      await f.page.getByPlaceholder("Search chats").fill("needle");
      await f.page.getByText("Conversation 29", { exact: true }).waitFor();
      await f.page
        .getByRole("button", {
          name: "Load earlier conversations",
          exact: true,
        })
        .click();
      await f.page.getByText("Conversation 34", { exact: true }).waitFor();
      assert.deepEqual(offsets, [
        ["needle", 0],
        ["needle", 30],
      ]);
    } finally {
      await f.close();
    }
  });
  await t.test("a late page cannot enter another knowledge base", async () => {
    const pending = deferred();
    const f = await open("/kb/one/chat", async (route, p, url) => {
      if (!p.endsWith("/conversations")) return;
      if (p.includes("/two/"))
        await route.fulfill({
          json: {
            conversations: [
              {
                ...rows(1)[0],
                id: "other",
                title: "Other library conversation",
              },
            ],
            total: 1,
          },
        });
      else if (url.searchParams.get("offset") === "30") pending.resolve(route);
      else
        await route.fulfill({ json: { conversations: rows(30), total: 65 } });
      return true;
    });
    try {
      await f.page.getByText("Conversation 29", { exact: true }).waitFor();
      await f.page
        .getByRole("button", {
          name: "Load earlier conversations",
          exact: true,
        })
        .click();
      const old = await pending.promise;
      await f.page.evaluate(() => window.__go("/kb/two/chat"));
      await f.page
        .getByText("Other library conversation", { exact: true })
        .waitFor();
      await old.fulfill({
        json: { conversations: rows(65).slice(30, 60), total: 65 },
      });
      await f.page.evaluate(() => new Promise(requestAnimationFrame));
      assert.equal(
        await f.page.getByText("Conversation 30", { exact: true }).count(),
        0,
      );
    } finally {
      await f.close();
    }
  });
  await t.test(
    "invalidation reloads all loaded pages after ordering changes",
    async () => {
      let list = rows(65);
      const refreshed = [];
      let moved = false;
      const f = await open("/kb/one/chat", async (route, p, url) => {
        if (!p.endsWith("/conversations")) return;
        const offset = Number(url.searchParams.get("offset"));
        if (moved) refreshed.push(offset);
        await route.fulfill({
          json: { conversations: list.slice(offset, offset + 30), total: 65 },
        });
        return true;
      });
      try {
        await f.page.getByText("Conversation 29", { exact: true }).waitFor();
        await f.page
          .getByRole("button", {
            name: "Load earlier conversations",
            exact: true,
          })
          .click();
        await f.page.getByText("Conversation 59", { exact: true }).waitFor();
        list = [list[64], ...list.slice(0, 64)];
        moved = true;
        await f.page.evaluate(() => window.__invalidate());
        await f.page.getByText("Conversation 64", { exact: true }).waitFor();
        assert.deepEqual(refreshed, [0, 30]);
        assert.equal(
          await f.page.getByText("Conversation 59", { exact: true }).count(),
          0,
        );
        assert.equal(
          await f.page
            .locator("aside")
            .getByText(/^Conversation \d+$/)
            .count(),
          60,
        );
      } finally {
        await f.close();
      }
    },
  );
});
