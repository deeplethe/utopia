import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });
for (const lang of ["en", "zh"]) {
  describe(lang, () => {
    it("identifies changed agent inputs by code without hiding unrelated failures", async () => {
      vi.stubGlobal("localStorage", { getItem: () => lang });
      const { ApiError } = await import("../api");
      const { S } = await import("../i18n");
      const { agentErrorMessage: message } = await import("./reviewErrors");
      expect(message(new ApiError(409, "server wording", "agent_inputs_changed"))).toBe(S.review.agentInputsChanged);
      expect(message(new ApiError(403, "not allowed"))).toBe("not allowed");
    });
    it("uses the stable busy code and gives other conflicts a distinct fallback", async () => {
      vi.stubGlobal("localStorage", { getItem: () => lang });
      const { ApiError } = await import("../api");
      const { S } = await import("../i18n");
      const { alignmentErrorMessage: message } = await import("./reviewErrors");
      expect(message(new ApiError(409, "changed server wording", "alignment_busy"))).toBe(S.review.alignmentKindWordBusy);
      for (const code of [undefined, "already_decided", "future_code"]) {
        expect(message(new ApiError(409, "busy, try again", code))).toBe(S.review.alignmentConflict);
        expect(message(new ApiError(409, "busy", code))).not.toBe(S.review.alignmentKindWordBusy);
      }
      for (const status of [401, 403, 404, 422, 500]) {
        expect(message(new ApiError(status, "original error", "alignment_busy"))).toBe("original error");
      }
    });
  });
}
