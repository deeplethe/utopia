import { Children, isValidElement, type ChangeEvent, type ReactNode } from "react";
import { expect, it, vi } from "vitest";
const state = vi.hoisted(() => ({ set: vi.fn() }));
vi.mock("react", async (original) => ({ ...(await original<typeof import("react")>()), useState: (initial: unknown) => [initial, state.set] }));
vi.mock("@tanstack/react-query", () => ({ useQuery: () => ({ data: { models_in_use: [{ base_url: "https://model.example", model: "fixture", kind: "llm" }] } }), useMutation: () => ({ mutate: vi.fn() }), useQueryClient: () => ({ invalidateQueries: vi.fn() }) }));
import { DeploymentAdmin } from "./Settings";
type Props = { children?: ReactNode; action?: ReactNode; type?: string; max?: number; value?: number; onChange?: (e: ChangeEvent<HTMLInputElement>) => void };
function inputs(node: ReactNode): Props[] {
  if (!isValidElement<Props>(node)) return [];
  return [...(node.props.type === "number" ? [node.props] : []), ...Children.toArray([node.props.children, node.props.action]).flatMap(inputs)];
}
it.each([0, 1, 2])("keeps concurrency input %s integral for the server integer fields", (index) => {
  state.set.mockClear();
  const input = inputs(DeploymentAdmin())[index];
  input.onChange!({ target: { value: "3.5" } } as ChangeEvent<HTMLInputElement>);
  const value = state.set.mock.calls[0][0];
  expect(typeof value === "number" ? value : value["https://model.example|fixture"]).toBe(3);
});
