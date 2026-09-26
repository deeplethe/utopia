import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { createServer } from "vite";
// From web/ after installing the app dependencies (no app dependency/lockfile changes):
// npm install --prefix /tmp/utopia-chat-browser-test --no-audit --no-fund --package-lock=false playwright-core@1.58.2
// CHAT_PLAYWRIGHT_PATH=/tmp/utopia-chat-browser-test/node_modules/playwright-core CHAT_CHROMIUM_PATH="/path/to/chromium" node --test tests/chat-view.test.mjs
// Set CHAT_CHROMIUM_PATH to an installed Chrome/Chromium executable, e.g.
// /Applications/Google Chrome.app/Contents/MacOS/Google Chrome on macOS.
// These on-demand Node browser tests are separate from pnpm test (Vitest) and CI.
const require = createRequire(import.meta.url);
const { chromium } = require(
  process.env.CHAT_PLAYWRIGHT_PATH || "playwright-core",
);
const root = fileURLToPath(new URL("../", import.meta.url));
const entry = `
import React from 'react'; import {createRoot} from 'react-dom/client';
import {QueryClient,QueryClientProvider} from '@tanstack/react-query';
import {createRouter,createRootRoute,createRoute,RouterProvider,Outlet} from '@tanstack/react-router';
import {Chat} from '/src/pages/Chat.tsx'; import {liveAnswer} from '/src/liveAnswer.ts'; import {ToastHost} from '/src/toast.tsx';
const parent=createRootRoute({component:Outlet});
const routes=['/kb/$kbId/chat','/kb/$kbId/chat/$conversationId'].map(path=>createRoute({getParentRoute:()=>parent,path,component:Chat}));
const router=createRouter({routeTree:parent.addChildren(routes)});
window.__go=to=>router.navigate({to}); window.__live=liveAnswer;
const client=new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
const tree=React.createElement(QueryClientProvider,{client},React.createElement(RouterProvider,{router}),React.createElement(ToastHost));
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
    async function open(path, custom, init) {
      const context = await browser.newContext();
      const page = await context.newPage();
      page.setDefaultTimeout(10000);
      // Runs in the page before the app, e.g. to replace browser APIs
      if (init) await page.addInitScript(init);
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
    await t.test(
      "idle reattachment rereads a just-completed history exactly once",
      async () => {
        let reads = 0,
          posts = 0;
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            reads++;
            await route.fulfill({
              json: {
                messages:
                  reads === 1
                    ? [message("Question", "user")]
                    : [
                        message("Question", "user"),
                        message("Saved between reads"),
                      ],
              },
            });
            return true;
          }
          if (p.endsWith("/chat") && route.request().method() === "POST") {
            posts++;
            return false;
          }
        });
        try {
          await f.page
            .getByText("Saved between reads", { exact: true })
            .waitFor();
          assert.equal(reads, 2);
          assert.equal(posts, 0);
        } finally {
          await f.close();
        }
      },
    );
    await t.test(
      "idle plus unanswered history is bounded without resending",
      async () => {
        let reads = 0;
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            reads++;
            await route.fulfill({
              json: { messages: [message("Still unanswered", "user")] },
            });
            return true;
          }
        });
        try {
          await f.page
            .getByText(
              "No active answer was found. You can send a new message.",
              { exact: true },
            )
            .waitFor();
          assert.equal(reads, 2);
          assert.deepEqual(f.errors, []);
        } finally {
          await f.close();
        }
      },
    );
    await t.test(
      "late idle refresh cannot overwrite another conversation",
      async () => {
        let reads = 0;
        const pending = deferred();
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            if (++reads === 1)
              await route.fulfill({
                json: { messages: [message("Question", "user")] },
              });
            else pending.resolve(route);
            return true;
          }
        });
        try {
          const refill = await Promise.race([
            pending.promise,
            new Promise((_, reject) =>
              setTimeout(
                () => reject(new Error("expected idle refresh request")),
                5000,
              ),
            ),
          ]);
          await f.page.evaluate(() => window.__go("/kb/one/chat/b"));
          await f.page.getByText("Answer beta", { exact: true }).waitFor();
          await refill.fulfill({
            json: { messages: [message("Obsolete saved answer")] },
          });
          await f.page.evaluate(() => new Promise(requestAnimationFrame));
          assert.equal(
            await f.page.getByText("Answer beta", { exact: true }).count(),
            1,
          );
          assert.equal(
            await f.page
              .getByText("Obsolete saved answer", { exact: true })
              .count(),
            0,
          );
        } finally {
          await f.close();
        }
      },
    );
    await t.test(
      "an idle refresh failure can be retried without a POST",
      async () => {
        let reads = 0;
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            reads++;
            await route.fulfill(
              reads === 2
                ? { status: 500, json: { error: "refresh failed" } }
                : {
                    json: {
                      messages:
                        reads === 1
                          ? [message("Question", "user")]
                          : [message("Recovered saved answer")],
                    },
                  },
            );
            return true;
          }
        });
        try {
          await f.page.getByRole("alert").waitFor();
          await f.page
            .getByRole("button", { name: "Retry", exact: true })
            .click();
          await f.page
            .getByText("Recovered saved answer", { exact: true })
            .waitFor();
          assert.equal(reads, 3);
          assert.deepEqual(f.errors, []);
        } finally {
          await f.close();
        }
      },
    );

    await t.test(
      "a pending idle refresh cannot overwrite a new send",
      async () => {
        const pending = deferred();
        let reads = 0;
        let posts = 0;
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            if (++reads === 1)
              await route.fulfill({
                json: { messages: [message("Earlier question", "user")] },
              });
            else pending.resolve(route);
            return true;
          }
          if (p.endsWith("/chat") && route.request().method() === "POST") {
            posts++;
            await route.fulfill({
              contentType: "text/event-stream",
              body: 'event: conversation\ndata: {"id":"a"}\n\nevent: delta\ndata: {"text":"New answer"}\n\nevent: done\ndata: {}\n\n',
            });
            return true;
          }
        });
        try {
          const old = await pending.promise;
          await f.page.getByPlaceholder("Ask anything…").fill("New question");
          await f.page.getByPlaceholder("Ask anything…").press("Enter");
          await f.page.getByText("New answer", { exact: true }).waitFor();
          await old.fulfill({
            json: { messages: [message("Obsolete refreshed answer")] },
          });
          await f.page.evaluate(() => new Promise(requestAnimationFrame));
          assert.equal(
            await f.page.getByText("New answer", { exact: true }).count(),
            1,
          );
          assert.equal(
            await f.page
              .getByText("Obsolete refreshed answer", { exact: true })
              .count(),
            0,
          );
          assert.equal(posts, 1);
        } finally {
          await f.close();
        }
      },
    );

    await t.test(
      "a reader who scrolled up stays put while an answer streams",
      async () => {
        const history = Array.from({ length: 40 }, (_, i) =>
          message(`Turn ${i}`, i % 2 ? "assistant" : "user"),
        );
        const f = await open("/kb/one/chat/a", async (route, p) => {
          if (p.endsWith("/conversations/a")) {
            await route.fulfill({ json: { messages: history } });
            return true;
          }
        });
        try {
          await f.page.getByText("Turn 39", { exact: true }).waitFor();
          const view = f.page.locator(".u-chat-fade");
          // This fixture loads no stylesheet, so the transcript is not a scroll box by
          // itself: give it the bounded height and overflow the app's layout gives it.
          await view.evaluate((el) => {
            el.style.height = "400px";
            el.style.overflowY = "auto";
          });
          // The answer grows through the same store a POST writes into.
          await f.page.evaluate(() => {
            const turns = Array.from({ length: 40 }, (_, i) => ({
              role: i % 2 ? "assistant" : "user",
              content: `Turn ${i}`,
            }));
            window.__answer = window.__live.begin(
              "one",
              "a",
              [
                ...turns,
                { role: "user", content: "And then?" },
                { role: "assistant", content: "" },
              ],
              () => {},
            );
          });
          const grow = async () => {
            for (let i = 0; i < 5; i++) {
              await f.page.evaluate(() =>
                window.__answer.patchLast((turn) => ({
                  ...turn,
                  content: `${turn.content}One more line of the answer.\n\n`,
                })),
              );
              await f.page.waitForTimeout(60);
            }
          };
          assert.ok(
            await view.evaluate((el) => el.scrollHeight > el.clientHeight + 200),
            "the transcript must be taller than the view",
          );
          // Scrolled up to read: the growing answer leaves the reader where they are.
          await view.evaluate((el) => {
            el.scrollTop = 0;
          });
          await f.page.waitForTimeout(60);
          await grow();
          assert.equal(await view.evaluate((el) => el.scrollTop), 0);
          // Back at the bottom: the answer is followed again.
          await view.evaluate((el) => {
            el.scrollTop = el.scrollHeight;
          });
          await f.page.waitForTimeout(60);
          await grow();
          assert.ok(
            await view.evaluate(
              (el) => el.scrollHeight - el.scrollTop - el.clientHeight <= 48,
            ),
            "a reader at the bottom follows the answer",
          );
          await f.page.evaluate(() => window.__answer.finish());
          assert.deepEqual(f.errors, []);
        } finally {
          await f.close();
        }
      },
    );
    const row = (id, title = `Conversation ${id}`) => ({
      id,
      title,
      created_at: "2026-01-01",
      updated_at: "2026-01-01",
    });
    await t.test("a delete that fails says so and keeps the conversation", async () => {
      const f = await open("/kb/one/chat/b", async (route, p) => {
        if (route.request().method() === "DELETE" && p.endsWith("/conversations/a")) {
          await route.fulfill({ status: 500, json: { error: "Could not delete it" } });
          return true;
        }
      });
      try {
        await f.page.getByText("Answer beta", { exact: true }).waitFor();
        await f.page.getByRole("button", { name: "More", exact: true }).first().click();
        await f.page.getByRole("menuitem", { name: "Delete conversation" }).click();
        await f.page.getByRole("button", { name: "Delete", exact: true }).click();
        await f.page.getByText("Could not delete it", { exact: true }).waitFor();
        assert.equal(
          await f.page.getByText("Conversation a", { exact: true }).count(),
          1,
        );
        assert.deepEqual(f.errors, []);
      } finally {
        await f.close();
      }
    });
    await t.test("a search keeps the listed conversations until its results arrive", async () => {
      const held = deferred();
      const f = await open("/kb/one/chat/b", async (route, p, url) => {
        if (p.endsWith("/conversations") && url.searchParams.get("q")) {
          held.resolve(route);
          return true;
        }
      });
      try {
        await f.page.getByText("Answer beta", { exact: true }).waitFor();
        await f.page.getByPlaceholder("Search chats").fill("alp");
        const search = await held.promise;
        for (const title of ["Conversation a", "Conversation b"])
          assert.equal(await f.page.getByText(title, { exact: true }).count(), 1, title);
        await search.fulfill({ json: { conversations: [row("a")], total: 1 } });
        await f.page
          .getByText("Conversation b", { exact: true })
          .waitFor({ state: "detached" });
        assert.equal(
          await f.page.getByText("Conversation a", { exact: true }).count(),
          1,
        );
        assert.deepEqual(f.errors, []);
      } finally {
        await f.close();
      }
    });
    await t.test("another library's conversations never stand in for this one's", async () => {
      const held = deferred();
      const f = await open("/kb/one/chat", async (route, p) => {
        if (p === "/api/v1/kbs/two/conversations") {
          held.resolve(route);
          return true;
        }
      });
      try {
        await f.page.getByText("Conversation a", { exact: true }).waitFor();
        await f.page.evaluate(() => window.__go("/kb/two/chat"));
        const list = await held.promise;
        assert.equal(
          await f.page.getByText("Conversation a", { exact: true }).count(),
          0,
        );
        await list.fulfill({
          json: { conversations: [row("c", "Other library chat")], total: 1 },
        });
        await f.page.getByText("Other library chat", { exact: true }).waitFor();
        assert.deepEqual(f.errors, []);
      } finally {
        await f.close();
      }
    });
    // A page served over https or localhost has navigator.clipboard
    const withApi = () => {
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText: async (text) => { window.__copied = text; } },
      });
    };
    // A base opened over plain http on a local network does not; record what
    // execCommand("copy") would copy: the selected text of a text area
    const withoutApi = () => {
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
      document.execCommand = (command) => {
        const area = [...document.querySelectorAll("textarea")].find(
          (a) => a.selectionEnd > a.selectionStart,
        );
        if (command !== "copy" || !area) return false;
        window.__copied = area.value.slice(area.selectionStart, area.selectionEnd);
        return true;
      };
    };
    await t.test("an answer and each of its code blocks can be copied, with or without the Clipboard API", async () => {
      const sql = "SELECT month, sum(amount) AS revenue FROM orders GROUP BY month";
      const answer = `Revenue by month:\n\n\`\`\`sql\n${sql}\n\`\`\`\n\nIt grew every month.`;
      for (const init of [withApi, withoutApi]) {
        const f = await open(
          "/kb/one/chat/a",
          async (route, p) => {
            if (p.endsWith("/conversations/a")) {
              await route.fulfill({ json: { messages: [message(answer)] } });
              return true;
            }
          },
          init,
        );
        try {
          await f.page.getByText("It grew every month.", { exact: true }).waitFor();
          await f.page.getByRole("button", { name: "Copy code", exact: true }).click();
          await f.page.getByRole("button", { name: "Copied", exact: true }).waitFor();
          assert.equal(await f.page.evaluate(() => window.__copied), sql, init.name);
          await f.page.getByRole("button", { name: "Copy answer", exact: true }).click();
          assert.equal(await f.page.evaluate(() => window.__copied), answer, init.name);
          assert.deepEqual(f.errors, [], init.name);
        } finally {
          await f.close();
        }
      }
    });
    await t.test("a query step says what it read and opens the SQL it ran", async () => {
      const sql =
        "SELECT month, sum(amount) AS revenue FROM orders GROUP BY month ORDER BY month";
      const f = await open("/kb/one/chat/a", async (route, p) => {
        if (p.endsWith("/conversations/a")) {
          await route.fulfill({
            json: {
              messages: [
                {
                  ...message("Revenue grew every month."),
                  steps: [
                    { kind: "query", label: "warehouse", detail: "revenue by month",
                      status: "ok", count: 12, sql, at: 0 },
                    { kind: "query", label: "warehouse", detail: "revenue by region",
                      status: "failed", sql: "SELECT region FROM nowhere", at: 0 },
                  ],
                },
              ],
            },
          });
          return true;
        }
      });
      try {
        await f.page
          .getByText("· revenue by month · 12 rows", { exact: true })
          .waitFor();
        const failed = f.page.getByText("· revenue by region · failed", {
          exact: true,
        });
        assert.match(await failed.getAttribute("class"), /\btext-danger\b/);
        assert.equal(await f.page.getByText(sql, { exact: true }).count(), 0);
        const toggles = f.page.getByRole("button", { name: "SQL", exact: true });
        assert.equal(await toggles.count(), 2);
        await toggles.first().click();
        await f.page.getByText(sql, { exact: true }).waitFor();
        assert.equal(await toggles.first().getAttribute("aria-expanded"), "true");
        assert.equal(
          await f.page.getByText("SELECT region FROM nowhere", { exact: true }).count(),
          0,
        );
        assert.deepEqual(f.errors, []);
      } finally {
        await f.close();
      }
    });
    await t.test("a conversation's title is copied from its menu, with or without the Clipboard API", async () => {
      for (const init of [withApi, withoutApi]) {
        const f = await open("/kb/one/chat/b", undefined, init);
        try {
          await f.page.getByText("Answer beta", { exact: true }).waitFor();
          await f.page.getByRole("button", { name: "More", exact: true }).first().click();
          await f.page.getByRole("menuitem", { name: "Copy title" }).click();
          // The copy says so, and the title is what reached the clipboard
          await f.page.getByText("Copied", { exact: true }).waitFor();
          assert.equal(await f.page.evaluate(() => window.__copied), "Conversation a", init.name);
          assert.deepEqual(f.errors, [], init.name);
        } finally {
          await f.close();
        }
      }
    });
  },
);
