// `fmtInterval` 实体侧栏里把一条事实的有效区间画成 "yyyy-mm-dd ~ now" /
// "ended, date unknown" / 单点的纯函数——2024 那条 PR（#669 之前）已经在
// 区分 "endedUnknown" 与 "ongoing"；现在把 "event"（一个时刻）补上：
// 之前显示 "2024-03-15 ~ 2024-03-15"（写端把 from/to 折成同一个时刻后
// UI 还把它当区间画），现在按 from 单独返回。

import { describe, expect, it, vi } from "vitest";

// 与 classHierarchy.test.ts 同：Graph.tsx 顶层 import 了 sigma，sigma 启动时
// 试着拿 WebGL2RenderingContext，vitest 默认跑在 jsdom 上没这个全局，模块
// 求值期就抛。Polyfill 即可，不必真渲染
vi.hoisted(() => {
  for (const name of ["WebGLRenderingContext", "WebGL2RenderingContext"]) {
    Object.defineProperty(globalThis, name, {
      configurable: true,
      value: class WebGLRenderingContext {},
    });
  }
});

import { fmtInterval } from "./Graph";
import type { EntityFact } from "../api";

const base: EntityFact = {
  id: "f",
  direction: "out",
  predicate_key: null,
  predicate_label: null,
  inferred: false,
  temporal: null,
  other_id: null,
  other_name: null,
  object_value: null,
  qualifiers: [],
  valid_from: null,
  valid_to: null,
  holds_from: null,
  holds_to: null,
  valid_from_precision: null,
  valid_to_precision: null,
  confidence: 1,
  evidence_count: 1,
  stale: false,
  corrected: false,
  contested: null,
  last_evidence_time: null,
};

const isoDay = "2024-03-15T00:00:00Z";

describe("fmtInterval", () => {
  it("eternal 不画区间", () => {
    expect(fmtInterval({ ...base, temporal: "eternal", valid_from: isoDay, valid_to: isoDay })).toBe(
      "",
    );
  });

  it("event 画一个时刻（from == to 的 day 精度）", () => {
    expect(
      fmtInterval({
        ...base,
        temporal: "event",
        valid_from: isoDay,
        valid_from_precision: "day",
        valid_to: isoDay,
        valid_to_precision: "day",
      }),
    ).toBe("2024-03-15");
  });

  it("event 单 hour 精度按 from 时刻返回", () => {
    expect(
      fmtInterval({
        ...base,
        temporal: "event",
        valid_from: "2024-03-15T10:30:00Z",
        valid_from_precision: "minute",
        valid_to: "2024-03-15T10:30:00Z",
        valid_to_precision: "minute",
      }),
    ).toBe("2024-03-15T10:30Z");
  });

  it("event 没有 from 时退到 to", () => {
    expect(
      fmtInterval({
        ...base,
        temporal: "event",
        valid_from: null,
        valid_to: "2024-03-15T00:00:00Z",
        valid_to_precision: "day",
      }),
    ).toBe("2024-03-15");
  });

  it("event from/to 都没有时返回空串（抽取失败不画）", () => {
    expect(fmtInterval({ ...base, temporal: "event" })).toBe("");
  });

  it("state 正常区间：from ~ ongoing（valid_to 为 null）", () => {
    expect(
      fmtInterval({
        ...base,
        temporal: "state",
        valid_from: "2020-01-01T00:00:00Z",
        valid_from_precision: "day",
        valid_to: null,
        valid_to_precision: null,
      }),
    ).toBe("2020-01-01 ~ now");
  });

  it("state 闭环：from ~ to", () => {
    expect(
      fmtInterval({
        ...base,
        temporal: "state",
        valid_from: "2020-01-01T00:00:00Z",
        valid_from_precision: "day",
        valid_to: "2024-01-01T00:00:00Z",
        valid_to_precision: "day",
      }),
    ).toBe("2020-01-01 ~ 2024-01-01");
  });

  it("state endedUnknown：from ~ ended, date unknown", () => {
    expect(
      fmtInterval({
        ...base,
        temporal: "state",
        valid_from: "2020-01-01T00:00:00Z",
        valid_from_precision: "day",
        valid_to: null,
        valid_to_precision: "unknown",
      }),
    ).toBe("2020-01-01 ~ ended, date unknown");
  });

  it("state 什么都没有时返回空串", () => {
    expect(fmtInterval({ ...base, temporal: "state" })).toBe("");
  });
});