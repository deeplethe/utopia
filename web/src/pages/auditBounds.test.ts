import { beforeAll, expect, it } from "vitest";
import { auditBounds } from "./KbSettings";

// 上海的 10 月 24 日是 UTC 的 23 日 16:00 到 24 日 16:00
beforeAll(() => {
  process.env.TZ = "Asia/Shanghai";
});

it("filters the activity log by the days the reader picked, end day included", () => {
  expect(auditBounds("2026-10-24", "2026-10-24")).toEqual({
    since: "2026-10-23T16:00:00.000Z",
    until: "2026-10-24T16:00:00.000Z",
  });
  // 截止日是月末：下一天是下个月的 1 日
  expect(auditBounds("", "2026-10-31").until).toBe("2026-10-31T16:00:00.000Z");
  expect(auditBounds("", "")).toEqual({ since: undefined, until: undefined });
});
