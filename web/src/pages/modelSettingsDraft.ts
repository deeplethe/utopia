import type { LlmSettingsView } from "../api";

export function savedModelFields(saved: LlmSettingsView | undefined) {
  return {
    chat_base_url: saved?.chat_base_url ?? "",
    chat_model: saved?.chat_model ?? "",
    chat_reasoning_effort: saved?.chat_reasoning_effort ?? "",
    chat_api_key: "",
    embed_base_url: saved?.embed_base_url ?? "",
    embed_model: saved?.embed_model ?? "",
    embed_api_key: "",
    ocr_provider: saved?.ocr_provider ?? "mineru",
    ocr_base_url: saved?.ocr_base_url ?? "",
    ocr_api_key: "",
    ocr_backend: saved?.ocr_backend ?? "",
    ocr_model: saved?.ocr_model ?? "",
    transcribe_base_url: saved?.transcribe_base_url ?? "",
    transcribe_api_key: "",
    transcribe_model: saved?.transcribe_model ?? "",
  };
}

type Field = keyof ReturnType<typeof savedModelFields>;
export type ModelDraft = Partial<Record<Field, string>>;
export type ModelCard = "chat" | "embed" | "ocr" | "transcribe";
export type ModelDrafts = Partial<Record<ModelCard, ModelDraft>>;

export function clearSubmittedDraft(
  drafts: ModelDrafts,
  card: ModelCard,
  submitted: ModelDraft | undefined,
): ModelDrafts {
  // 每次编辑都会替换这张卡的草稿；旧请求完成时不能清掉后来改的内容。
  if (!submitted || drafts[card] !== submitted) return drafts;
  const remaining = { ...drafts };
  delete remaining[card];
  return remaining;
}
