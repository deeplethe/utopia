# Ingest interfaces

Utopia pulls or receives documents through **sources**. Two source kinds speak JSON and are meant for integration: **Custom** (Utopia polls your service) and **API** (your service pushes to Utopia). Both share the same identity semantics: every item has a stable ID, and pushing or returning the same ID again **updates the same document in place** — the previous content is kept as a version, search re-indexes, and the knowledge graph re-extracts.

## Choosing between them

| | Custom (pull) | API (push) |
|---|---|---|
| Who initiates | Utopia, on a schedule | Your service, any time |
| Auth | Optional header you configure | Per-source Bearer token |
| Fits | Feeds, exports, periodic snapshots | Event-driven systems, scripts, CI |
| Deletion signal | `deleted` array in the response | `deleted: true` in a push |

---

## RSS sources

RSS sources keep the existing `rss` source kind and can run in either mode:

- **Feed content only** (`content_mode: "feed"`) preserves compatibility. Utopia stores the feed body or summary and does not request the linked article.
- **Full article content for new items** (`content_mode: "full_new_items"`) is opt-in. The first successful sync records the current feed as a baseline and imports no documents. Later entries are discovered durably and hydrated in the background.

Full-content hydration prefers substantive feed-native HTML (`content:encoded`) and converts it to safe Markdown. If that is absent or too thin, Utopia fetches the first alternate HTTP(S) article link through a bounded SSRF-resistant client, applies readability extraction, and converts the result to Markdown. A failed linked fetch or summary-only entry stays retryable or terminal with a typed diagnostic; it is never marked as completed full content. The source bar shows labeled pending, queued, retrying, complete, and terminal counts; per-entry troubleshooting data remains internal.

Only newly observed entries after activation are eligible for hydration. Changing from feed mode to full mode, or changing the feed URL while full mode is active, starts a new baseline generation; changing back stops new hydration work but preserves the ledger history. Browser rendering, authenticated pages, cookies, paywall bypass, challenge solving, recursive links, assets, and transcription are not part of this path.

---

## Custom source — the pull interface

Create a **Custom** source and point it at any URL you control. On every sync (manual, interval, or cron) Utopia sends:

```
GET {endpoint}?since=2026-08-26T19:43:24Z
Authorization: <your configured header, if any>
```

- `since` is the last successful sync time (RFC 3339). It is **omitted on the first sync** — return everything then; afterwards you may return only what changed since that moment. Returning unchanged items again is harmless: they are skipped by content hash.
- If you configured an *Authorization header* on the source, it is sent verbatim in the `Authorization` header. It is stored server-side and never shown again in any API response.

Respond with JSON:

```json
{
  "items": [
    {
      "id": "note-42",
      "title": "Deployment runbook",
      "content": "# Runbook\nRestart the ingest worker before each release.",
      "doc_time": "2026-08-20T09:00:00Z",
      "mime": "text/markdown"
    }
  ],
  "deleted": ["note-17"]
}
```

Field reference:

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Stable identity within this source. Same `id` + new `content` → the document is updated in place. |
| `title` | no | Display name; the file extension is inferred from `mime` when missing. Defaults to `id`. |
| `content` | yes | Full text of the item (not a diff). |
| `doc_time` | no | RFC 3339. Becomes the document's time axis position — feed the real publish/effective time whenever you have it. |
| `mime` | no | `text/markdown` (default), `text/plain`, or `text/html`. |
| `deleted` | no | Array of `id`s your source has retired. Matching documents are **marked "Not in source"**, never auto-deleted — a person decides in the Library. Returning an item again clears the mark. |

Notes:

- Items missing from a response are **not** treated as deleted (the response may be incremental). Only the `deleted` array signals retirement.
- Endpoints on `localhost` are fetched directly, bypassing any system HTTP proxy.

---

## API source — the push interface

Create an **API** source; it gets its own push token (view or rotate it from the source's Token dialog). Then:

```
POST {your-utopia-base}/api/v1/sources/{source_id}/ingest
Authorization: Bearer utp_…
Content-Type: application/json
```

```json
{
  "filename": "runbook.md",
  "content": "# Runbook\nRestart the ingest worker before each release.",
  "doc_time": "2026-08-20T09:00:00Z",
  "external_id": "note-42"
}
```

| Field | Required | Meaning |
|---|---|---|
| `filename` | yes | Display name; also the fallback identity when `external_id` is absent. |
| `content` | yes* | Full text. *Optional when `deleted` is `true`.* |
| `doc_time` | no | RFC 3339; the document's position on the time axis. |
| `external_id` | no | Stable identity. Same identity + new content → update in place. Without it, `filename` is the identity. |
| `deleted` | no | `true` marks the identified document "Not in source" (tombstone). A later normal push of the same identity revives it. |

The response tells you what happened:

```json
{ "action": "created" }
```

`created` · `updated` · `moved` (same content, new name) · `unchanged` · `marked_missing`.

Example with curl:

```bash
curl -X POST "https://utopia.example.com/api/v1/sources/01a0…/ingest" \
  -H "Authorization: Bearer utp_…" \
  -H "Content-Type: application/json" \
  -d '{"filename":"runbook.md","content":"# Runbook v2 …","external_id":"note-42"}'
```

---

## Shared semantics

- **Identity, not filenames.** Documents are tracked by `custom:{id}` / `api:{external_id}` keys. Renames are recognized as moves; content changes update the same document.
- **Updates keep history.** Every content change records a version; earlier extracted knowledge keeps its provenance.
- **Deletion is a marker.** Tombstones set a "Not in source" flag; the Library shows a cleanup action, and a human confirms actual deletion.
- **`doc_time` drives the time axis.** Documents without it fall back to their ingestion time — real timestamps make the temporal graph meaningfully better.
