/** 轨迹上的一步怎么说（#942）。
 *
 *  服务端每一步只给字段（0004）：`status`、数、时刻，外加个别步骤自己的几样。
 *  话在这里按读者的语言说。`label` 与问数、remember 的 `detail` 是数据，原样显示。
 *
 *  **没有 `status` 的是这之前存下的消息**，它们只有服务端当时写的那句英文，
 *  原样显示；不去解析那句英文再说一遍。 */
import type { ChatStep } from "./api";
import { S, type Strings } from "./i18n";
import { fmtTime } from "./time";
import { localDateTime } from "./ui";

/** 世界时间：日历上的日子，按 UTC 读（见 fmtTime）。带了钟点的才写钟点 */
function worldMoment(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const precision =
    d.getUTCSeconds() || d.getUTCMilliseconds()
      ? "second"
      : d.getUTCHours() || d.getUTCMinutes()
        ? "minute"
        : "day";
  return fmtTime(iso, precision) ?? iso;
}

/** 记录时间：一个真实的时刻，按看的人所在的时区（见 localDateTime） */
function recordMoment(iso: string): string {
  return Number.isNaN(new Date(iso).getTime()) ? iso : localDateTime(iso);
}

/** 这一步的对象。只有 changes 要改：服务端拼的「2026-09-01 → now」里 now 是英文 */
export function stepLabel(step: ChatStep, T: Strings = S): string {
  if (step.status && step.kind === "changes" && step.since) {
    return `${step.since} → ${step.until ?? T.ask.step.now}`;
  }
  return step.label;
}

/** 这一步的结果 */
export function stepDetail(step: ChatStep, T: Strings = S): string {
  const W = T.ask.step;
  if (!step.status) return step.detail;
  // 问数的 detail 是这次查询的目的，结果跟在它后面
  const withPurpose = (result: string) =>
    step.kind === "query" && step.detail ? `${step.detail} · ${result}` : result;
  if (step.status === "failed") return withPurpose(W.failed);
  if (step.status === "not_found") {
    if (step.kind === "document") return W.documentNotFound;
    if (step.kind === "query") return withPurpose(W.sourceNotFound);
    if (step.kind === "tool") return W.unknownTool;
    return W.entityNotFound;
  }
  if (step.status === "invalid") {
    if (!step.param) return W.unparsed;
    return step.missing ? W.missing(step.param) : W.invalid(step.param);
  }
  const n = step.count ?? 0;
  const more = step.more === true;
  switch (step.kind) {
    case "search":
      return W.sources(n);
    case "document":
    case "docs":
      return W.sections(n);
    case "entity":
      return W.matches(n, step.total);
    case "facts": {
      const record = step.before
        ? W.recordedBefore(recordMoment(step.before))
        : step.as_of
          ? W.recordedBy(recordMoment(step.as_of))
          : null;
      return W.facts(n, step.valid_at ? worldMoment(step.valid_at) : null, record);
    }
    case "neighbors":
      return W.linked(n, step.total ?? n);
    case "timeline":
      return W.dated(n, step.total ?? n);
    case "path":
      return W.paths(n, more, step.hops ?? 0);
    case "changes":
      return W.changes(n, more);
    case "query":
      return withPurpose(W.rows(n, more));
    case "tool":
      if (step.label === "list_rules") return W.rules(n);
      if (step.label === "rule_matches") return W.marked(n);
      // remember 的 detail 是记下的那句话
      return step.detail;
    default:
      // 以后的服务端多出来的 kind：它的 detail 仍是一句能读的英文
      return step.detail;
  }
}
