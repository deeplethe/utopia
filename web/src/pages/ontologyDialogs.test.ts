import { createElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { RelationTypeView } from "../api";
import { PropertyDialog } from "./ontologyDialogs";

// Radix 的 Portal 在服务端渲染里什么都不出，弹窗的壳换成直接出正文
vi.mock("../ui", async (original) => ({
  ...(await original<typeof import("../ui")>()),
  FormDialog: ({ children }: { children: ReactNode }) => children,
}));

// 只填弹窗用得上的字段，没填的它按缺省处理（`existing?.x ?? …`）
const def = (id: string, kind: RelationTypeView["kind"], label: string) =>
  ({ id, key: id, label, kind, usage: 0 }) as RelationTypeView;

describe("PropertyDialog", () => {
  it("offers the knowledge base's attributes as edge attributes", () => {
    // 已选的显示成标签，标签的名字从候选里取：候选里没有这个属性，标签就出不来
    const relation = { ...def("moves_to", "relation", "Moves to"), qualifiers: ["reason"] };
    const html = renderToStaticMarkup(createElement(QueryClientProvider, {
      client: new QueryClient(),
      children: createElement(PropertyDialog, {
        kbId: "kb", existing: relation, allTypes: [], allRelations: [relation],
        allAttributes: [def("reason", "attribute", "Reason")], initialDomain: null,
        onClose: vi.fn(), onSaved: vi.fn(), onDeleted: vi.fn(), onError: vi.fn(),
      }),
    }));
    expect(html).toContain("Reason");
  });
});
