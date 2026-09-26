import { describe, expect, it } from "vitest";
import { FOLLOW_SLACK_PX, followsBottom } from "./chatScroll";

/** 视口 600、内容 2000：滚到 1400 就是底 */
const view = (scrollTop: number, clientHeight = 600, scrollHeight = 2000) => ({
  scrollTop,
  clientHeight,
  scrollHeight,
});

describe("following a streaming answer", () => {
  it("a reader at the bottom follows the answer", () => {
    expect(followsBottom(view(1400))).toBe(true);
  });

  it("a reader within the slack still counts as at the bottom", () => {
    expect(followsBottom(view(1400 - FOLLOW_SLACK_PX))).toBe(true);
  });

  it("a reader who scrolled up stays where they are", () => {
    expect(followsBottom(view(1400 - FOLLOW_SLACK_PX - 1))).toBe(false);
    expect(followsBottom(view(1200))).toBe(false);
    expect(followsBottom(view(0))).toBe(false);
  });

  it("a conversation shorter than the view has nowhere to scroll away to", () => {
    expect(followsBottom(view(0, 600, 400))).toBe(true);
  });
});
