import type { AlignmentItem, AlignmentMarks, RelationTypeView } from "../api";

type PhraseItem = Extract<AlignmentItem, { kind: "phrase" }>;

/** 一条短语签名打开时预选什么。人绑过、只差 marks 的按现在的绑定；否则按第一票、第二票。
 *  没有票说过单个日期标什么就取「都不是」：只带一个日期的陈述留在开放图谱，是不猜的
 *  那个读法（#966） */
export function phraseStart(item: PhraseItem): {
  property: string;
  direction: "forward" | "reverse";
  marks: AlignmentMarks;
} {
  const first = item.votes?.first ?? null;
  const second = item.votes?.second ?? null;
  return {
    property: item.bound_to ?? first?.property ?? second?.property ?? "",
    direction: item.direction ?? first?.direction ?? second?.direction ?? "forward",
    marks: first?.marks ?? second?.marks ?? "none",
  };
}

/** 选中的属性是状态时才问单个日期标什么：事件与恒常没有这一问，服务端也不收 */
export function asksMarks(properties: RelationTypeView[], key: string): boolean {
  return properties.find((p) => p.key === key)?.temporal === "state";
}
