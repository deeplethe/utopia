import { expect, it } from "vitest";
import { driveFolderAccountKey, reconcileDriveFolderDraft } from "./googleDriveFolderDraft";

const connection = {
  connected_at: "2026-11-01T00:00:00Z",
  connected_by: "editor",
  account_email: "first@example.com",
};
const newGrant = { ...connection, connected_at: "2026-11-14T00:00:00Z" };
const saved = [{ id: "old", name: "Already saved" }];
const draft = {
  accountKey: driveFolderAccountKey("kb", "source", connection),
  folders: [...saved, { id: "new", name: "Unsaved" }],
};
const reconcile = (config: Parameters<typeof driveFolderAccountKey>[2]) =>
  reconcileDriveFolderDraft(driveFolderAccountKey("kb", "source", config), draft, saved);

it("keeps unsaved folders across refreshes and reconnects to the same verified account", () => {
  expect(reconcile({ ...connection })).toEqual(draft);
  expect(reconcile({ ...newGrant, account_email: "FIRST@EXAMPLE.COM" })).toEqual(draft);
  expect(reconcileDriveFolderDraft(draft.accountKey, draft, draft.folders)).toBeNull();
});

it("drops a draft when the account changes or the source is disconnected", () => {
  expect(reconcile({ ...newGrant, account_email: "second@example.com" })).toBeNull();
  expect(reconcile({})).toBeNull();
});

it("uses the grant timestamp when Google provides no verified email", () => {
  const unidentified = { ...connection, account_email: undefined };
  const accountKey = driveFolderAccountKey("kb", "source", unidentified);
  const unidentifiedDraft = { ...draft, accountKey };
  expect(reconcileDriveFolderDraft(accountKey, unidentifiedDraft, saved)).toEqual(unidentifiedDraft);
  const next = driveFolderAccountKey("kb", "source", { ...newGrant, account_email: undefined });
  expect(reconcileDriveFolderDraft(next, unidentifiedDraft, saved)).toBeNull();
});
