import type { AlignmentItem, AlignmentMarks, RelationTypeView } from "../api";

type PhraseItem = Extract<AlignmentItem, { kind: "phrase" }>;

/** 一条短语签名打开时预选什么。绑上了、只差 marks 的按现在的绑定；否则按第一票、第二票。
 *  单个日期标什么**不预选**：升级后成批进来的绑定若预选一个读法，一次点击就替人写下了它
 *  （#975 评审）。状态属性下人得自己选一个，才能绑 */
export function phraseStart(item: PhraseItem): {
  property: string;
  direction: "forward" | "reverse";
  marks: AlignmentMarks | null;
} {
  const first = item.votes?.first ?? null;
  const second = item.votes?.second ?? null;
  return {
    property: item.bound_to ?? first?.property ?? second?.property ?? "",
    direction: item.direction ?? first?.direction ?? second?.direction ?? "forward",
    marks: null,
  };
}

/** 选中的属性是状态时才问单个日期标什么：事件与恒常没有这一问，服务端也不收 */
export function asksMarks(properties: RelationTypeView[], key: string): boolean {
  return properties.find((p) => p.key === key)?.temporal === "state";
}
