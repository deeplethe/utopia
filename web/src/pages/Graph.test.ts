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

import {
  fmtInterval,
  proofIndentClass,
  PROOF_INDENT_CAP,
  walkProofSteps,
} from "./Graph";
import type { EntityFact, ProofStep } from "../api";

const base: EntityFact = {
  id: "f",
  said_as: null,
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

// 递归证明树（0030）的形状：服务端 proof() 返回的 ProofStep 现在带
// premises: ProofStep[]，前端把它压平成 WalkedRow[] 喂给渲染组件。
// 这里断言三件事：
//   1. 深度单调 + 跨过 has_premises 边界才 +1（叶子的 premises 为空就停）
//   2. 三层嵌套的 9-节点树产出 9 行，深度序列为 [0,1,2,3,2,3,1,2,3]，
//      即 DFS 前序：父 → 第一个子 → 第一个孙 → … → 第一个叶 → 兄弟 →
//   3. 叶子行的 has_premises=false；不是叶子的行它必为 true
//       A
//      / \
//     B   C
//    / \   \
//   D   E   F
//   |   |   |
//   G   H   I   ← 叶子（premises 为空，walker 不下钻）

function step(
  id: string,
  premises: ProofStep[] = [],
  partial: Partial<ProofStep> = {},
): ProofStep {
  return {
    seq: 0,
    fact_id: id,
    subject_id: id,
    subject: id,
    predicate_id: null,
    predicate: "→",
    object_id: null,
    object: id,
    valid_from: null,
    valid_to: null,
    confidence: 1,
    retracted: false,
    evidence: [],
    premises,
    ...partial,
  };
}

describe("walkProofSteps", () => {
  it("叶子（premises 为空）只有一个深度 0 的行", () => {
    const tree = [step("leaf")];
    const rows = walkProofSteps(tree);
    expect(rows).toHaveLength(1);
    expect(rows[0].depth).toBe(0);
    expect(rows[0].has_premises).toBe(false);
    expect(rows[0].fact_id).toBe("leaf");
  });

  it("三层嵌套的树产出 9 行，深度 0..3 单调", () => {
    const tree = [
      step("A", [
        step("B", [step("D", [step("G")]), step("E", [step("H")])]),
        step("C", [step("F", [step("I")])]),
      ]),
    ];
    const rows = walkProofSteps(tree);
    expect(rows.map((r) => r.fact_id)).toEqual([
      "A", "B", "D", "G", "E", "H", "C", "F", "I",
    ]);
    expect(rows.map((r) => r.depth)).toEqual([
      0, 1, 2, 3, 2, 3, 1, 2, 3,
    ]);
    // 深度从不超过 4（一个 `premises` 边只 +1）；从不低于 0；
    // 同一行 `premises` 内的 depth 差只可能是 +1（刚下钻）或非正（爬回祖先或平移到同层）
    for (const r of rows) {
      expect(r.depth).toBeGreaterThanOrEqual(0);
      expect(r.depth).toBeLessThanOrEqual(3);
    }
    // 进入 premises 时 +1，退出时不强制 -1（可以一次回到祖先）
    for (let i = 1; i < rows.length; i++) {
      const diff = rows[i].depth - rows[i - 1].depth;
      expect(diff).toBeLessThanOrEqual(1);
    }
  });

  it("叶子行 has_premises=false；非叶子行必为 true", () => {
    const tree = [
      step("A", [step("B", [step("leaf")])]),
    ];
    const rows = walkProofSteps(tree);
    const byId = Object.fromEntries(rows.map((r) => [r.fact_id, r]));
    expect(byId["A"].has_premises).toBe(true);
    expect(byId["B"].has_premises).toBe(true);
    expect(byId["leaf"].has_premises).toBe(false);
  });

  it("深度起点偏移：depth=2 时整棵树每个节点的深度都比直接调用多 2", () => {
    const tree = [step("A", [step("B")])];
    const at0 = walkProofSteps(tree, 0);
    const at2 = walkProofSteps(tree, 2);
    expect(at0.map((r) => r.depth)).toEqual([0, 1]);
    expect(at2.map((r) => r.depth)).toEqual([2, 3]);
  });
});

// 缩进封顶：`cn` 是纯拼接、仓库里没有 tailwind-merge，所以同时发出 `ml-4`
// 与 `ml-0` 时谁生效由样式表顺序决定——那样的封顶形同虚设。这里钉住的是
// 「深到一定层数就不再发缩进类」，而不是「再发一个类把它盖掉」
describe("proofIndentClass", () => {
  it("封顶以内：带缩进、内边距与那条竖线", () => {
    for (let depth = 0; depth < PROOF_INDENT_CAP; depth++) {
      const cls = proofIndentClass(depth);
      expect(cls).toContain("ml-4");
      expect(cls).toContain("pl-3");
      expect(cls).toContain("border-l");
    }
  });

  it("到了封顶就不再发缩进类", () => {
    for (const depth of [PROOF_INDENT_CAP, PROOF_INDENT_CAP + 1, 12]) {
      const cls = proofIndentClass(depth);
      expect(cls).not.toContain("ml-4");
      expect(cls).not.toContain("pl-3");
      expect(cls).not.toContain("border-l");
    }
  });

  it("任何深度都不会同时发出互相冲突的两个类", () => {
    for (let depth = 0; depth <= 12; depth++) {
      const parts = proofIndentClass(depth).split(/\s+/).filter(Boolean);
      for (const [a, b] of [
        ["ml-4", "ml-0"],
        ["pl-3", "pl-0"],
        ["border-l", "border-l-0"],
      ]) {
        expect(parts.includes(a) && parts.includes(b)).toBe(false);
      }
      // 同一个属性组里也不该出现两个取值
      const margins = parts.filter((c) => /^ml-/.test(c));
      expect(margins.length).toBeLessThanOrEqual(1);
    }
  });

  it("每一层都留着 mt-1，封顶只去掉横向的缩进", () => {
    for (let depth = 0; depth <= 12; depth++) {
      expect(proofIndentClass(depth)).toContain("mt-1");
    }
  });
});