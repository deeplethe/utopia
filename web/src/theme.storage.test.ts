import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

describe("theme choice when storage is unavailable", () => {
  it("retains the user's choice in this session", async () => {
    vi.stubGlobal("localStorage", {
      getItem: () => { throw new Error("storage denied"); },
      setItem: () => { throw new Error("storage denied"); },
    });
    vi.stubGlobal("document", {
      documentElement: { dataset: {}, classList: { toggle: vi.fn() } },
    });
    const theme = await import("./theme");
    theme.setTheme("light");
    expect(theme.getTheme()).toBe("light");
    expect(theme.resolvedTheme()).toBe("light");
  });
});
