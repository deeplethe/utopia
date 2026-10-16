import type { Kb } from "../api";

export type KbGroupItem = { kb: Kb; label: string };
export type KbEntry =
  | { kind: "kb"; kb: Kb }
  | { kind: "group"; name: string; items: KbGroupItem[] };

/** 名字带 `/` 的库按第一个 `/` 之前的部分归组（`re-cinq/lore` → 组 `re-cinq`、
 *  标签 `lore`）。两边去掉空白后都得有字，且同组至少两个库才成组——`/foo`、
 *  `foo/`、只此一家的 `CI/CD` 照旧是一行全名。组落在它第一个成员原来的位置，
 *  其余库的顺序不动 */
export function kbGroups(kbs: Kb[]): KbEntry[] {
  const groups = new Map<string, KbGroupItem[]>();
  const owners = kbs.map((kb) => {
    const split = ownerAndLabel(kb.name);
    if (!split) return undefined;
    groups.set(split.owner, [...(groups.get(split.owner) ?? []), { kb, label: split.label }]);
    return split.owner;
  });
  const placed = new Set<string>();
  return kbs.flatMap((kb, index): KbEntry[] => {
    const owner = owners[index];
    const items = owner === undefined ? undefined : groups.get(owner)!;
    if (!items || items.length < 2) return [{ kind: "kb", kb }];
    if (placed.has(owner!)) return [];
    placed.add(owner!);
    return [{ kind: "group", name: owner!, items }];
  });
}

function ownerAndLabel(name: string) {
  const slash = name.indexOf("/");
  const owner = name.slice(0, slash).trim();
  const label = name.slice(slash + 1).trim();
  return slash >= 0 && owner && label ? { owner, label } : undefined;
}

/** 给多选下拉用的选项：同组的挨在一起，`label` 留全名（搜索与已选的芯片都用它），
 *  `short` 是组名下面那一行显示的名字 */
export function kbOptions(kbs: Kb[]) {
  return kbGroups(kbs).flatMap((entry) =>
    entry.kind === "kb"
      ? [{ value: entry.kb.id, label: entry.kb.name }]
      : entry.items.map((item) => ({
          value: item.kb.id,
          label: item.kb.name,
          group: entry.name,
          short: item.label,
        })),
  );
}
