import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { ApiError } from "../api";
import { S } from "../i18n";
import { KbActivity } from "./KbSettings";

const kbId = "research";
const queryKey = ["kbAudit", kbId, "", "", "", 0];
const empty = { events: [], total: 0, actions: [] };

function createClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false, retryOnMount: false, refetchOnMount: false } },
  });
}

function render(client: QueryClient) {
  return renderToStaticMarkup(createElement(QueryClientProvider, {
    client,
    children: createElement(KbActivity, { kbId }),
  }));
}

it("shows loading without a zero count before the first response", () => {
  const html = render(createClient());
  expect(html).toContain(S.nav.loading);
  expect(html).not.toContain(S.kbset.auditTotal(0));
  expect(html).not.toContain(S.kbset.activityEmpty);
});

it.each([false, true])("offers retry after a 503 (previous empty response: %s)", async (cached) => {
  const client = createClient();
  if (cached) client.setQueryData(queryKey, empty);

  const error = new ApiError(503, "Service Unavailable");
  await expect(client.fetchQuery({
    queryKey,
    queryFn: async () => { throw error; },
  })).rejects.toBe(error);

  const html = render(client);
  expect(html).toContain('role="alert"');
  expect(html).toContain(S.kbset.activityLoadError);
  expect(html).toContain(S.kbset.activityRetry);
  expect(html).not.toContain(S.kbset.auditTotal(0));
  expect(html).not.toContain(S.kbset.activityEmpty);
});

it("shows zero records after a successful empty response", () => {
  const client = createClient();
  client.setQueryData(queryKey, empty);
  const html = render(client);
  expect(html).toContain(S.kbset.auditTotal(0));
  expect(html).toContain(S.kbset.activityEmpty);
  expect(html).not.toContain('role="alert"');
});
