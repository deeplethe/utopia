import {
  Children, isValidElement, useState, type ChangeEvent, type ComponentProps,
  type JSXElementConstructor, type ReactNode,
} from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api, type Kb } from "../api";
import { S } from "../i18n";
import { Button, Checkbox, Input, Segmented, SettingsCard } from "../ui";
import { KbSettings } from "./KbSettings";

// Node 中执行真实组件与保存回调；查询快照模拟刷新边界，不覆盖 TanStack 缓存集成。
const hooks = vi.hoisted(() => ({
  kbId: "kb-a",
  data: undefined as Kb | undefined,
  query: undefined as { queryFn: () => Promise<Kb> } | undefined,
  invalidate: vi.fn(),
  states: [] as unknown[],
  dependencies: [] as Array<readonly unknown[]>,
  effects: [] as Array<() => void>,
  stateIndex: 0,
  effectIndex: 0,
  changed: false,
  saving: undefined as Promise<unknown> | undefined,
}));
vi.mock("react", async (original) => ({
  ...await original<typeof import("react")>(),
  useState: (initial: unknown) => {
    const index = hooks.stateIndex++;
    if (!(index in hooks.states)) hooks.states[index] = initial;
    return [hooks.states[index], (value: unknown) => {
      hooks.changed ||= !Object.is(hooks.states[index], value);
      hooks.states[index] = value;
    }];
  },
  useEffect: (effect: () => void, dependencies: readonly unknown[]) => {
    const previous = hooks.dependencies[hooks.effectIndex];
    hooks.dependencies[hooks.effectIndex++] = dependencies;
    if (!previous || dependencies.some((value, index) => !Object.is(value, previous[index]))) {
      hooks.effects.push(effect);
    }
  },
}));
vi.mock("@tanstack/react-router", () => ({
  useParams: () => ({ kbId: hooks.kbId }),
  useNavigate: () => vi.fn(),
}));
vi.mock("@tanstack/react-query", () => ({
  useQueryClient: () => ({ invalidateQueries: hooks.invalidate }),
  useQuery: (query: { queryKey: string[]; queryFn: () => Promise<Kb> }) => {
    if (query.queryKey[0] !== "kbOne") return { data: { failed: 0 } };
    hooks.query = query;
    return { data: hooks.data, isPending: !hooks.data };
  },
  useMutation: (options: {
    mutationFn: (patch: Record<string, unknown>) => Promise<unknown>;
    onSuccess: (result: unknown) => void;
  }) => {
    const [status, setStatus] = useState({ isPending: false, isSuccess: false });
    return {
      ...status,
      mutate: (patch: Record<string, unknown>) => {
        setStatus({ isPending: true, isSuccess: false });
        hooks.saving = options.mutationFn(patch).then((result) => {
          options.onSuccess(result);
          setStatus({ isPending: false, isSuccess: true });
        });
      },
    };
  },
}));

const initial: Kb = {
  id: "kb-a", workspace_id: "workspace", name: "Research", kind: "general",
  description: "Original description", visibility: "open", is_default: false,
  auto_extend_ontology: false, materialize_inferences: false,
  auto_type_resolution: false, governance: false, governance_since: null,
  inference_interval_minutes: 60, last_inference_at: null, ontology_lang: "en",
};
let server: Kb;

beforeEach(() => {
  server = { ...initial };
  Object.assign(hooks, {
    kbId: initial.id, data: undefined, query: undefined,
    states: [], dependencies: [], effects: [], saving: undefined,
  });
  hooks.invalidate.mockReset();
  vi.spyOn(api, "kbDetail").mockImplementation(async () => ({ ...server }));
  vi.spyOn(api, "updateKb").mockImplementation(async (_id, patch) => {
    server = { ...server, ...patch };
    return server;
  });
});
afterEach(() => vi.restoreAllMocks());

async function refresh() {
  render();
  expect(hooks.query).toBeDefined();
  hooks.data = await hooks.query!.queryFn();
}

function render(): ReactNode {
  hooks.stateIndex = hooks.effectIndex = 0;
  hooks.changed = false;
  const tree = KbSettings();
  for (const effect of hooks.effects.splice(0)) effect();
  return hooks.changed ? render() : tree;
}

function find<P>(node: ReactNode, type: JSXElementConstructor<P> | string): P[] {
  if (!isValidElement<{ children?: ReactNode; action?: ReactNode }>(node)) return [];
  // JSX 的 type 与组件一致时，props 就是该组件的公开属性。
  const matched = node.type === type ? [node.props as P] : [];
  return [...matched, ...Children.toArray([node.props.children, node.props.action])
    .flatMap((child) => find(child, type))];
}

function card(title: string) {
  const found = find(render(), SettingsCard).find((item) => item.title === title);
  expect(found, title).toBeDefined();
  return found!;
}

function editIdentity(name: string, description: string) {
  const inputs = find(card(S.kbset.cardIdentity).children, Input);
  for (const [index, value] of [name, description].entries()) {
    inputs[index].onChange!({ target: { value } } as ChangeEvent<HTMLInputElement>);
  }
}

async function save(title: string) {
  const button = find(card(title).action, Button)[0];
  expect(button.disabled).toBe(false);
  button.onClick!({} as Parameters<NonNullable<ComponentProps<typeof Button>["onClick"]>>[0]);
  await hooks.saving;
  expect(hooks.invalidate.mock.calls).toEqual([
    [{ queryKey: ["kbOne", hooks.kbId] }], [{ queryKey: ["kbs"] }],
  ]);
  await refresh();
}

function expectSaved(title: string) {
  const action = card(title).action;
  expect(find(action, Button)[0].disabled).toBe(true);
  expect(find<{ children?: ReactNode }>(action, "span")
    .some((span) => span.children === S.kbset.saved)).toBe(true);
}

it.each(["Original description", "", null])(
  "keeps identity drafts when only visibility is saved (description: %s)",
  async (description) => {
    server.description = description;
    await refresh();
    editIdentity("Draft name", "Draft description");
    find(card(S.settings.kbs.visibility).children, Segmented)[0].onChange("restricted");
    await save(S.settings.kbs.visibility);

    expect(api.updateKb).toHaveBeenCalledExactlyOnceWith("kb-a", { visibility: "restricted" });
    expect(api.kbDetail).toHaveBeenCalledTimes(2);
    expect(find(card(S.kbset.cardIdentity).children, Input).map((input) => input.value))
      .toEqual(["Draft name", "Draft description"]);
    expect(find(card(S.settings.kbs.visibility).children, Segmented)[0].value).toBe("restricted");
    expectSaved(S.settings.kbs.visibility);
  },
);

it("synchronizes saved identity values and status while preserving other cards' drafts", async () => {
  await refresh();
  editIdentity("  New name  ", "  New description  ");
  find(card(S.settings.kbs.visibility).children, Segmented)[0].onChange("restricted");
  find(card(S.kbset.cardAutomation).children, Checkbox)[0].onChange!(true);
  find(card(S.kbset.ontologyLang).children, Segmented)[0].onChange("zh");
  await save(S.kbset.cardIdentity);

  expect(api.updateKb).toHaveBeenLastCalledWith("kb-a", { name: "New name", description: "New description" });
  expect(find(card(S.kbset.cardIdentity).children, Input).map((input) => input.value))
    .toEqual(["New name", "New description"]);
  expectSaved(S.kbset.cardIdentity);
  expect(find(card(S.settings.kbs.visibility).children, Segmented)[0].value).toBe("restricted");
  expect(find(card(S.kbset.cardAutomation).children, Checkbox)[0].checked).toBe(true);
  expect(find(card(S.kbset.ontologyLang).children, Segmented)[0].value).toBe("zh");
});

it("accepts server updates on unedited cards", async () => {
  await refresh();
  render();
  server = {
    ...server, name: "Renamed remotely", description: "Updated remotely", visibility: "restricted",
    materialize_inferences: true, auto_type_resolution: true, governance: true,
    inference_interval_minutes: 120, ontology_lang: "zh",
  };
  await refresh();
  expect(find(card(S.kbset.cardIdentity).children, Input).map((input) => input.value))
    .toEqual([server.name, server.description]);
  expect(find(card(S.settings.kbs.visibility).children, Segmented)[0].value).toBe("restricted");
  expect(find(card(S.kbset.cardAutomation).children, Checkbox).map((input) => input.checked))
    .toEqual([true, true, true]);
  expect(find(card(S.kbset.cardAutomation).children, Input)[0].value).toBe(120);
  expect(find(card(S.kbset.ontologyLang).children, Segmented)[0].value).toBe("zh");
});

it.each([false, true])("resets drafts when switching KB with identical server values (preloaded: %s)", async (preloaded) => {
  await refresh();
  editIdentity("Draft name", "Draft description");
  find(card(S.settings.kbs.visibility).children, Segmented)[0].onChange("restricted");
  find(card(S.kbset.cardAutomation).children, Checkbox)[0].onChange!(true);
  find(card(S.kbset.ontologyLang).children, Segmented)[0].onChange("zh");
  render();
  hooks.kbId = "kb-b";
  server = { ...initial, id: "kb-b" };
  hooks.data = preloaded ? { ...server } : undefined;
  render();
  await refresh();
  expect(find(card(S.kbset.cardIdentity).children, Input).map((input) => input.value))
    .toEqual([initial.name, initial.description]);
  expect(find(card(S.settings.kbs.visibility).children, Segmented)[0].value).toBe("open");
  expect(find(card(S.kbset.cardAutomation).children, Checkbox).map((input) => input.checked))
    .toEqual([false, false, false]);
  expect(find(card(S.kbset.ontologyLang).children, Segmented)[0].value).toBe("en");
});
