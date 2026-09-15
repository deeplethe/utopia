import { S } from "./i18n";

/** 一块文字从哪来（0040）。原文是常态，常态不标；只有模型读出来、转写出来、描述出来的
 *  那几种才在证据和文档里写一句——人据此知道这句话可能认错一个字、听错一个名字，
 *  或者根本没有人这么写过 */
export type Origin = "stated" | "ocr" | "transcribed" | "described";
export type Anchor = Record<string, unknown> | null;

const num = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) ? v : null;

/** 毫秒写成录音里的钟点：一小时以内 m:ss，超过 h:mm:ss */
export function clock(ms: number): string {
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** 出处的一句短话：「OCR · p. 2」「Transcribed · 0:06–0:09 · A, B」。原文返回 null */
export function originLabel(origin?: string | null, anchor?: Anchor): string | null {
  switch (origin) {
    case "ocr":
      return S.origin.ocr(num(anchor?.page));
    case "transcribed": {
      const start = num(anchor?.start_ms);
      const end = num(anchor?.end_ms);
      const speakers = Array.isArray(anchor?.speaker)
        ? anchor.speaker.filter((s): s is string => typeof s === "string")
        : typeof anchor?.speaker === "string"
          ? [anchor.speaker]
          : [];
      return S.origin.transcribed(
        start !== null && end !== null ? `${clock(start)}–${clock(end)}` : null,
        speakers,
      );
    }
    case "described":
      return S.origin.described;
    default:
      return null;
  }
}

/** 悬停说明：这种出处要当心什么，以及是哪个模型读的 */
export function originHint(origin?: string | null, model?: string | null): string | undefined {
  const why =
    origin === "ocr"
      ? S.origin.ocrHint
      : origin === "transcribed"
        ? S.origin.transcribedHint
        : origin === "described"
          ? S.origin.describedHint
          : null;
  if (!why) return undefined;
  return model ? `${why} ${S.origin.readBy(model)}` : why;
}
