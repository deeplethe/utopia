import { Children, isValidElement, type ChangeEvent, type ReactNode } from "react";
import { expect, it, vi } from "vitest";
const state = vi.hoisted(() => ({ set: vi.fn() }));
vi.mock("react", async (original) => ({ ...(await original<typeof import("react")>()), useState: (initial: unknown) => [initial, state.set] }));
vi.mock("@tanstack/react-query", () => ({ useQuery: () => ({ data: undefined }), useMutation: () => ({ mutate: vi.fn() }), useQueryClient: () => ({ invalidateQueries: vi.fn() }) }));
import { DeploymentAdmin } from "./Settings";
type Props = { children?: ReactNode; action?: ReactNode; type?: string; max?: number; value?: number; onChange?: (e: ChangeEvent<HTMLInputElement>) => void };
function inputs(node: ReactNode): Props[] {
  if (!isValidElement<Props>(node)) return [];
  return [...(node.props.type === "number" ? [node.props] : []), ...Children.toArray([node.props.children, node.props.action]).flatMap(inputs)];
}
it("offers the full server worker backstop range and its fallback", () => {
  const worker = inputs(DeploymentAdmin())[0];
  expect(worker.max).toBe(256);
  expect(worker.value).toBe(64);
  worker.onChange!({ target: { value: "128" } } as ChangeEvent<HTMLInputElement>);
  expect(state.set).toHaveBeenCalledWith(128);
});
