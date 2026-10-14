# 0068 · A Google Drive source is connected by a person

- **Status**: Implemented · 2026-10-14 · migration 0112 · tried by hand against a real Drive; the test fixtures are written from the documentation, not recorded (0013 decision 4) · open: revision history, change-feed sync
- **Written**: 2026-10-14 (conventions in the [README](README.md))
- **Related**: [0013](0013-a-source-should-hand-over-its-history.md) (what a source must pass, fixtures from real responses); [0014](0014-identity-from-the-person-scope-from-the-token.md) (identity comes from the person); migration `0056` (`oidc_flows`, the shape the connection flow copies); [#246](https://github.com/deeplethe/utopia/issues/246) (a credential never reaches a viewer)

## Problem

Most teams keep their documents in Google Drive. Today the way in is to export them by hand, or to copy them into a bucket and sync that, which loses each file's identity and its modification time. Drive passes the tests of 0013: `modifiedTime` is the content's own clock, documents are rewritten in place, a file id survives renames and moves, and the knowledge actually lives there.

Every existing connector is configured with a key an administrator pastes in. Drive has no such key for a person's files. A service account sees only what is shared with it, so every folder would first have to be shared with a robot address, and domain-wide delegation lets one key read everyone's Drive, far more than anyone chose to include. The natural grant is the one a person gives: "read these folders of mine, as me".

## Decisions

1. **A person connects the source with OAuth, and it reads Drive as them.** An editor of the knowledge base opens the source's settings and clicks Connect. Google's consent screen asks for `drive.readonly` plus `openid email`, and the refresh token is stored in the source's config under `refresh_token`, which is in `SOURCE_SECRET_KEYS`: sealed at rest, never in a listing, never in a response (#246). The browser never holds a Google token. Even the folder browser asks the server, which reads Drive with the stored grant. `drive.file` was not enough: it covers only files the app created or the person picked one by one, and picking a folder does not include what is inside it.

2. **The OAuth client belongs to the deployment.** `UTOPIA_GOOGLE_DRIVE_CLIENT_ID`, `_CLIENT_SECRET` and `_REDIRECT_URI` name a Google "web application" client the operator registers. When any of the three is missing, `GET /google-drive/status` says so, the kind cannot be created from the dialog, and a sync fails with that sentence rather than a 401. This client is separate from the SSO client of 0056. The two ask for different scopes, and one deployment may sign people in elsewhere and still read Drive.

3. **The flow is bound to the person who started it.** `POST …/google-drive/connect` records a single-use `state`, a PKCE verifier, the user and the source in `google_drive_flows` (ten minutes, at most 1000 pending), and returns Google's URL. The callback deletes the row before using it, requires the browser's session to be that same user, and checks again that the user is still an editor. Without the session check, someone could start a flow for their own source and get another person to click through consent, and the other person's Drive would be connected to their source. `prompt=consent` and `access_type=offline` are always sent, so a reconnect after revocation gets a new refresh token.

4. **The ID token names the account, and that is all it does.** `openid email` exists so the settings can say "connected as bruno@…". The ID token comes straight from Google's token endpoint over TLS, so issuer and audience are checked and the signature is not (OIDC Core 3.1.3.7). An unverified email is not shown. The email never identifies a Utopia account and never links one; that stays as 0056 decided. `connected_at` in the config is what the interface reads as "connected", because the email may legitimately be absent.

5. **The person chooses folders, and subfolders come with them.** `config.folders` is `[{id, name}]`. The name is only for the interface. The browser lists My Drive, Shared with me and every shared drive. Saving the selection (`PUT …/google-drive/folders`) queues a sync at once. Ids are checked against `[A-Za-z0-9_-]` before they go into a Drive query, which is simpler to trust than escaping. The limit is 50 folders, 16 levels, 2000 files and 32 MB per file, the same order as WebDAV and object storage. Hitting any of them is reported, not silent.

6. **Google formats are exported to Office formats.** Docs become DOCX, Sheets XLSX and Slides PPTX, and the existing parsers read them with headings, tables and every sheet. Plain text or CSV would have lost the headings that 0039's chunks and 0064's section dates depend on, and CSV keeps only the first sheet. Forms, drawings, shortcuts and sites have no readable body and are skipped. Everything else is downloaded as bytes with `application/octet-stream`, because the parser trusts magic bytes over names, as object storage already does.

7. **Change is detected from Drive's marker, not from the bytes.** An export of an unchanged Doc is a new zip with new timestamps, so a content hash changes on every run, and every run would re-extract every Doc. `google_drive_files` keeps one marker per file: `md5Checksum` for binary files, and `modifiedTime` for Google formats, which have no checksum. When the marker is unchanged and the document is live, the file is not downloaded at all.

8. **Retraction follows a complete walk only.** Files not seen are marked "not in source" (`reconcile_missing`) only when every selected folder opened and no limit was hit. A folder that returns 404, because it was deleted or is no longer shared with the connected person, fails the run by name and marks nothing. Not being able to look is not the same as finding it empty. A file that cannot be read (Google refuses exports over 10 MB) keeps its stored document, the rest of the run continues, and the run is reported as failed with the names, as Notion does.

9. **Disconnecting forgets the grant and asks Google to revoke it.** `DELETE …/google-drive/connection` removes the token from the config first and then calls Google's revoke endpoint. If the revoke fails, that is logged, and the token is already gone from Utopia. Connecting and disconnecting are audited with the account email and never the token.

## Dead ends

- **A service account with folders shared to it.** It is robust, and nobody's departure breaks it. But selecting a folder becomes "share it with a robot address and paste an id", and a shared drive must add the robot as a member. It may still be worth having for organisations that will not tie a sync to one person; it would be a second way to fill the same `refresh_token`-shaped slot, not a different source.
- **The Google Picker.** It needs a browser API key and an app id beside the OAuth client, puts a Google token in the browser, and with `drive.file` would still not grant a folder's contents. A server-side folder list costs one endpoint and keeps decision 1 intact.
- **Comparing content hashes**, as every other connector does. That costs a re-extraction of every Google Doc on every run (decision 7).
- **The changes feed** (`changes.list` with a saved page token). It is cheaper on large drives, but it reports changes across the whole Drive and not under a folder, so each change needs an ancestry walk, and moves out of a selected folder are the hard case. The full walk is simple and bounded. Revisit when a deployment hits the 2000-file limit.

## Open questions

- **Revision history.** Drive keeps revisions, and 0013 prefers a source that hands its history over. Exporting old revisions of Google formats is possible but expensive; which revisions to sample is its own decision, as it is for Feishu and Confluence.
- **When the person leaves.** The grant is theirs. When their account is suspended the refresh token stops working, and the sync fails with "reconnect the source". That is honest, but someone has to notice it. A `document.needs_reader`-style alert for a dead grant would make it visible.
- **Verification by Google.** `drive.readonly` is a restricted scope. An "internal" OAuth client in a Workspace organisation needs no verification; an "external" one used beyond a hundred test users needs Google's security assessment. The operator decides that, but the setup guide should say so.
