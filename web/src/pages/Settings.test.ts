import { describe, expect, it } from "vitest";
import { modelCardStatus } from "./Settings";

describe("model card status", () => {
  it("prefers a fresh test result over a stale save confirmation", () => {
    // #698：测完显示的一直是 "Saved"，分不清这一句说的是保存还是测试
    expect(
      modelCardStatus(
        { ok: true, message: "Reachable and authenticated (OK)" },
        null,
        { error: null, saved: true, dirty: false },
      ),
    ).toEqual({
      kind: "note",
      tone: "text-ok",
      text: "Reachable and authenticated (OK)",
    });
  });

  it("surfaces a failed test instead of the previous save state", () => {
    expect(
      modelCardStatus(
        { ok: false, message: "Not configured" },
        null,
        { error: null, saved: true, dirty: false },
      ),
    ).toEqual({ kind: "note", tone: "text-danger", text: "Not configured" });
  });

  it("reports a transport-level test failure rather than going quiet", () => {
    // 从前请求本身没通时卡上什么都不说，旧的 "Saved" 还贴着
    expect(
      modelCardStatus(null, "Network Error", { error: null, saved: true, dirty: false }),
    ).toEqual({ kind: "note", tone: "text-danger", text: "Network Error" });
  });

  it("keeps save errors and confirmations when nothing was tested", () => {
    expect(
      modelCardStatus(null, null, { error: "401 Unauthorized", saved: false, dirty: false }),
    ).toEqual({
      kind: "note",
      tone: "text-danger",
      text: "401 Unauthorized",
    });
    expect(
      modelCardStatus(null, null, { error: null, saved: true, dirty: false }),
    ).toEqual({ kind: "saved" });
  });

  it("says unsaved when the card was edited, whatever the last test said", () => {
    // #698：改了密钥直接点测试，看到的是旧配置的「已连通」
    expect(
      modelCardStatus(
        { ok: true, message: "Reachable and authenticated (OK)" },
        null,
        { error: null, saved: false, dirty: true },
      ),
    ).toEqual({ kind: "unsaved" });
    // 保存失败的报错仍然先说
    expect(
      modelCardStatus(null, null, { error: "401 Unauthorized", saved: false, dirty: true }),
    ).toEqual({ kind: "note", tone: "text-danger", text: "401 Unauthorized" });
  });

  it("stays silent when nothing happened yet", () => {
    expect(
      modelCardStatus(null, null, { error: null, saved: false, dirty: false }),
    ).toEqual({ kind: "idle" });
  });
});
