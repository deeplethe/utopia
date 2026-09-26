import { afterEach, describe, expect, it, vi } from "vitest";
import { copyAndSay, copyText } from "./clipboard";
import { S } from "./i18n";
import { toast } from "./toast";

vi.mock("./toast", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

interface FakeArea {
  value: string;
  readOnly: boolean;
  attached: boolean;
  selected: boolean;
  style: Record<string, string>;
  setAttribute(name: string): void;
  focus(): void;
  select(): void;
  remove(): void;
}

/** node 里没有 DOM：一个只够 copyText 用的 document。`execCommand` 记下它被调的那一刻
 *  页面上选中的是哪段字——那正是浏览器会放进剪贴板的东西 */
function page(result: boolean | "throw") {
  const before = { focus: vi.fn() };
  const areas: FakeArea[] = [];
  const copied: (string | undefined)[] = [];
  vi.stubGlobal("document", {
    activeElement: before,
    body: {
      appendChild: (area: FakeArea) => {
        area.attached = true;
      },
    },
    createElement: (): FakeArea => {
      const area: FakeArea = {
        value: "",
        readOnly: false,
        attached: false,
        selected: false,
        style: {},
        setAttribute: (name) => {
          if (name === "readonly") area.readOnly = true;
        },
        focus: () => {},
        select: () => {
          area.selected = true;
        },
        remove: () => {
          area.attached = false;
        },
      };
      areas.push(area);
      return area;
    },
    execCommand: (command: string) => {
      if (command === "copy") copied.push(areas.find((a) => a.attached && a.selected)?.value);
      if (result === "throw") throw new Error("not allowed here");
      return result;
    },
  });
  return { before, areas, copied };
}

describe("copyText", () => {
  it("writes through the Clipboard API when the page has it", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const { copied } = page(true);
    expect(await copyText("SELECT 1")).toBe(true);
    expect(writeText).toHaveBeenCalledWith("SELECT 1");
    expect(copied).toEqual([]);
  });

  it("copies a selected text area where the API does not exist, as over plain http", async () => {
    vi.stubGlobal("navigator", {});
    const { before, areas, copied } = page(true);
    expect(await copyText("SELECT 1")).toBe(true);
    expect(copied).toEqual(["SELECT 1"]);
    // 那个看不见的 textarea 用完就拿掉，焦点还给原来那个元素
    expect(areas).toHaveLength(1);
    expect(areas[0].readOnly).toBe(true);
    expect(areas[0].attached).toBe(false);
    expect(before.focus).toHaveBeenCalled();
  });

  it("falls back the same way when the API refuses", async () => {
    vi.stubGlobal("navigator", {
      clipboard: { writeText: vi.fn().mockRejectedValue(new Error("denied")) },
    });
    const { copied } = page(true);
    expect(await copyText("SELECT 1")).toBe(true);
    expect(copied).toEqual(["SELECT 1"]);
  });

  it("says it did not copy when neither way works", async () => {
    for (const result of [false, "throw"] as const) {
      vi.stubGlobal("navigator", {});
      const { areas } = page(result);
      expect(await copyText("SELECT 1")).toBe(false);
      expect(areas.every((a) => !a.attached)).toBe(true);
    }
  });
});

describe("copyAndSay", () => {
  it("says what was copied when the copy worked", async () => {
    vi.stubGlobal("navigator", {});
    page(true);
    expect(await copyAndSay("ingest-token", "Token copied")).toBe(true);
    expect(toast.success).toHaveBeenCalledWith("Token copied");
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("says it could not copy when neither way worked", async () => {
    vi.stubGlobal("navigator", {});
    page(false);
    expect(await copyAndSay("ingest-token", "Token copied")).toBe(false);
    expect(toast.error).toHaveBeenCalledWith(S.toast.copyFailed);
    expect(toast.success).not.toHaveBeenCalled();
  });
});
