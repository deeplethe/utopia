import { ApiError } from "../api";
import { S } from "../i18n";

/** A 409 says conflict, not which operation conflicted. Never infer busy from prose. */
export function alignmentErrorMessage(error: unknown): string {
  if (error instanceof ApiError && error.status === 409) {
    return error.code === "alignment_busy"
      ? S.review.alignmentKindWordBusy
      : S.review.alignmentConflict;
  }
  return (error as Error).message;
}
