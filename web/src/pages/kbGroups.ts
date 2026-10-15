import type { Kb } from "../api";

export type KbGroupItem = { kb: Kb; label: string };
export type KbEntry =
  | { kind: "kb"; kb: Kb }
  | { kind: "group"; name: string; items: KbGroupItem[] };

/** 名字带 `/` 的库按第一个 `/` 之前的部分归组（`re-cinq/lore` → 组 `re-cinq`、
 *  标签 `lore`）。组落在它第一个成员原来的位置，其余库的顺序不动 */
export function kbGroups(kbs: Kb[]): KbEntry[] {
  const entries: KbEntry[] = [];
  const groups = new Map<string, KbGroupItem[]>();
  for (const kb of kbs) {
    const slash = kb.name.indexOf("/");
    if (slash < 0) {
      entries.push({ kind: "kb", kb });
      continue;
    }
    const name = kb.name.slice(0, slash);
    const item = { kb, label: kb.name.slice(slash + 1) };
    const items = groups.get(name);
    if (items) {
      items.push(item);
      continue;
    }
    groups.set(name, [item]);
    entries.push({ kind: "group", name, items: groups.get(name)! });
  }
  return entries;
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
