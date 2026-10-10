import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

describe("workspace selection without browser storage", () => {
  it("still imports and notifies selections when reads and writes are denied", async () => {
    vi.stubGlobal("localStorage", {
      getItem: () => { throw new Error("storage denied"); },
      setItem: () => { throw new Error("storage denied"); },
    });
    const { wsStore } = await import("./wsStore");
    const listener = vi.fn();
    wsStore.subscribe(listener);
    wsStore.set("workspace");
    expect(wsStore.get()).toBe("workspace");
    expect(listener).toHaveBeenCalledOnce();
  });
});
