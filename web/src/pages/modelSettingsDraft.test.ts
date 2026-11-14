import { expect, it } from "vitest";
import { clearSubmittedDraft, savedModelFields } from "./modelSettingsDraft";

it("only discards the saved card's draft when the server normalizes back to the old value", () => {
  const saved = savedModelFields({ embed_model: "embed-model" });
  const chat = { chat_model: "draft-chat", chat_api_key: "chat-draft-key" };
  const embed = { embed_model: " embed-model " };
  const remaining = clearSubmittedDraft({ chat, embed }, "embed", embed);

  expect(remaining).toEqual({ chat });
  expect({ ...saved, ...remaining.embed }.embed_model).toBe("embed-model");
});

it("keeps edits made after the submitted draft was captured", () => {
  const submitted = { embed_model: "first", embed_api_key: "first-key" };
  const newer = { embed_model: "second", embed_api_key: "second-key" };
  const drafts = { embed: newer };
  expect(clearSubmittedDraft(drafts, "embed", submitted)).toBe(drafts);
  expect(clearSubmittedDraft(drafts, "embed", newer)).toEqual({});
});
