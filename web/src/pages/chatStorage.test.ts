import { afterEach, describe, expect, it, vi } from "vitest";
import { Children, isValidElement, type ChangeEvent, type ReactNode } from "react";

vi.mock("react", async (original) => {
  const react = await original<typeof import("react")>();
  return { ...react, useState: (initial: unknown) => [typeof initial === "function" ? initial() : initial, vi.fn()],
    useEffect: vi.fn(), useLayoutEffect: vi.fn(), useRef: (current: unknown) => ({ current }),
    useSyncExternalStore: (_subscribe: unknown, get: () => unknown) => get() };
});
vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: undefined }), useInfiniteQuery: () => ({ data: undefined }),
  useMutation: () => ({ mutate: vi.fn() }), useQueryClient: () => ({ invalidateQueries: vi.fn() }),
}));
vi.mock("@tanstack/react-router", () => ({ useNavigate: () => vi.fn(), useParams: () => ({}) }));
vi.mock("../kb", () => ({ useKbId: () => "kb", useKb: () => ({ kb: { id: "kb", name: "KB" }, kbs: [] }) }));
afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

function findComposer(node: ReactNode): { onChange?: (event: ChangeEvent<HTMLTextAreaElement>) => void } | null {
  if (!isValidElement<{ children?: ReactNode; onChange?: (event: ChangeEvent<HTMLTextAreaElement>) => void }>(node)) return null;
  if (node.props.onChange && "rows" in node.props && node.props.rows === 1) return node.props;
  for (const child of Children.toArray(node.props.children)) {
    const found = findComposer(child);
    if (found) return found;
  }
  return null;
}

describe("chat without session storage", () => {
  it("can open its composer when draft reads are denied", async () => {
    vi.stubGlobal("sessionStorage", { getItem: () => { throw new Error("storage denied"); } });
    const { Chat } = await import("./Chat");
    expect(() => Chat()).not.toThrow();
  });
  it("keeps the composer change handler usable when writes are denied", async () => {
    vi.stubGlobal("sessionStorage", {
      getItem: () => null,
      setItem: () => { throw new Error("storage denied"); },
    });
    const { Chat } = await import("./Chat");
    const composer = findComposer(Chat());
    expect(composer?.onChange).toBeTypeOf("function");
    expect(() => composer?.onChange?.({ target: { value: "hello" }, currentTarget: { style: {}, scrollHeight: 20 } } as unknown as ChangeEvent<HTMLTextAreaElement>)).not.toThrow();
  });
});
