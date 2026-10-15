import type { Kb } from "../api";

export type KbEntry = { kind: "kb"; kb: Kb };

export function kbGroups(kbs: Kb[]): KbEntry[] {
  return kbs.map((kb) => ({ kind: "kb", kb }));
}
