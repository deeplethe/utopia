import { beforeAll, describe, expect, it } from "vitest";
import type { ChatStep } from "./api";
import { en } from "./i18n/en";
import { zh } from "./i18n/zh";
import { stepDetail, stepLabel } from "./steps";

// UTC 与本地读出来不是同一天、同一钟点的时区：世界时间与记录时间用错了读法就看得出来
beforeAll(() => {
  process.env.TZ = "America/New_York";
});

const step = (s: Partial<ChatStep>): ChatStep => ({
  kind: "tool",
  label: "",
  detail: "",
  ...s,
});

describe("stepDetail", () => {
  it("shows the stored English of a step saved before the fields existed", () => {
    const old = step({ kind: "facts", label: "Acme", detail: "12 facts as of 2024-08-01T00:00:00Z" });
    expect(stepDetail(old, en)).toBe("12 facts as of 2024-08-01T00:00:00Z");
    expect(stepDetail(old, zh)).toBe("12 facts as of 2024-08-01T00:00:00Z");
    expect(stepLabel(step({ kind: "changes", label: "2026-09-01 → now" }), zh)).toBe(
      "2026-09-01 → now",
    );
  });

  it.each<[Partial<ChatStep>, string, string]>([
    [{ kind: "search", status: "ok", count: 6 }, "6 sources", "6 个来源"],
    [{ kind: "search", status: "ok", count: 1 }, "1 source", "1 个来源"],
    [{ kind: "document", status: "ok", count: 3 }, "3 sections", "3 段"],
    [{ kind: "docs", status: "ok", count: 0 }, "0 sections", "0 段"],
    [{ kind: "entity", status: "ok", count: 3 }, "3 matches", "3 个匹配"],
    [{ kind: "entity", status: "ok", count: 5, total: 12 }, "5 of 12 matches", "5 个匹配（共 12 个）"],
    [{ kind: "facts", status: "ok", count: 12 }, "12 facts", "12 条事实"],
    [{ kind: "neighbors", status: "ok", count: 3, total: 10 }, "3 of 10 linked", "关联 10 个，列出 3 个"],
    [{ kind: "neighbors", status: "ok", count: 4, total: 4 }, "4 linked", "关联 4 个"],
    [{ kind: "timeline", status: "ok", count: 2, total: 5 }, "2 of 5 dated facts", "5 条有日期的事实，列出 2 条"],
    [{ kind: "path", status: "ok", count: 0 }, "no path", "没有路径"],
    [{ kind: "path", status: "ok", count: 1, hops: 2 }, "1 path, shortest 2 hops", "1 条路径，最短 2 跳"],
    [{ kind: "path", status: "ok", count: 5, hops: 1, more: true }, "5+ paths, shortest 1 hop", "5+ 条路径，最短 1 跳"],
    [{ kind: "changes", status: "ok", count: 0 }, "no changes", "没有变更"],
    [{ kind: "changes", status: "ok", count: 40, more: true }, "40+ changes", "40+ 处变更"],
    [{ kind: "tool", label: "list_rules", status: "ok", count: 0 }, "none", "无"],
    [{ kind: "tool", label: "list_rules", status: "ok", count: 2 }, "2 rules", "2 条规则"],
    [{ kind: "tool", label: "rule_matches", status: "ok", count: 7 }, "7 marked", "标出 7 处"],
    [{ kind: "search", status: "failed" }, "failed", "失败"],
    [{ kind: "document", status: "not_found" }, "document not found", "未找到文档"],
    [{ kind: "neighbors", status: "not_found" }, "entity not found", "未找到实体"],
    [{ kind: "tool", label: "lookup", status: "not_found" }, "unknown tool", "未知工具"],
    [{ kind: "tool", label: "search_chunks", status: "invalid" }, "incomplete arguments", "参数不完整"],
    [{ kind: "tool", status: "invalid", param: "query", missing: true }, "missing query", "缺少 query"],
    [{ kind: "document", status: "invalid", param: "document_id" }, "invalid document_id", "document_id 无效"],
  ])("words %j in both languages", (fields, english, chinese) => {
    // detail 故意给一句别的：有 status 的步骤不该读它
    const s = step({ detail: "server words", ...fields });
    expect(stepDetail(s, en)).toBe(english);
    expect(stepDetail(s, zh)).toBe(chinese);
  });

  it("keeps what belongs to the user or the model as written", () => {
    const remembered = step({ label: "remember", detail: "我们在 2026 年换了供应商", status: "ok" });
    expect(stepDetail(remembered, zh)).toBe("我们在 2026 年换了供应商");
    const query = step({ kind: "query", label: "warehouse", detail: "按月的收入", status: "ok", count: 12 });
    expect(stepLabel(query, zh)).toBe("warehouse");
    expect(stepDetail(query, zh)).toBe("按月的收入 · 12 行");
    expect(stepDetail(query, en)).toBe("按月的收入 · 12 rows");
    expect(stepDetail({ ...query, count: 200, more: true }, en)).toBe("按月的收入 · 200+ rows");
    expect(stepDetail({ ...query, status: "failed" }, zh)).toBe("按月的收入 · 失败");
    expect(stepDetail({ ...query, status: "not_found" }, en)).toBe("按月的收入 · no such data source");
  });

  it("builds the changes window in the reader's language", () => {
    const open = step({ kind: "changes", label: "2026-09-01 → now", status: "ok", count: 3, since: "2026-09-01" });
    expect(stepLabel(open, zh)).toBe("2026-09-01 → 现在");
    expect(stepLabel(open, en)).toBe("2026-09-01 → now");
    expect(stepLabel({ ...open, until: "2026-09-15" }, zh)).toBe("2026-09-01 → 2026-09-15");
  });

  it("reads the world moment as a calendar day and the record moment in local time", () => {
    // 这条测试靠的就是时区：没设上的话下面的断言什么也证明不了
    expect(new Date("2024-08-01T00:00:00Z").getDate()).toBe(31);
    const facts = step({
      kind: "facts",
      label: "Acme",
      status: "ok",
      count: 12,
      valid_at: "2024-08-01T00:00:00Z",
      before: "2026-09-12T06:03:00Z",
    });
    // 世界时间是 8 月 1 日，不是纽约的 7 月 31 日晚上；记录时刻是纽约的 02:03
    expect(stepDetail(facts, en)).toBe("12 facts at 2024-08-01, as recorded before 2026-09-12 02:03");
    expect(stepDetail(facts, zh)).toBe("2024-08-01 时的 12 条事实，按 2026-09-12 02:03 之前的记录");
    const asOf = step({ kind: "facts", status: "ok", count: 1, as_of: "2026-09-12T06:03:00Z" });
    expect(stepDetail(asOf, en)).toBe("1 fact, as recorded by 2026-09-12 02:03");
    // 带钟点的世界时间写到钟点，仍按 UTC
    const clocked = step({ kind: "facts", status: "ok", count: 2, valid_at: "2024-08-01T14:30:00Z" });
    expect(stepDetail(clocked, en)).toBe("2 facts at 2024-08-01T14:30Z");
  });
});
