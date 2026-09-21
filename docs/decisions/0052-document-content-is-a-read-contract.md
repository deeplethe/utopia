# 0052 · Document content is a read contract over the retained ledger

- **Status**: proposed for review
- **Written**: 2026-09-21
- **Related**: [#859](https://github.com/deeplethe/utopia/issues/859); [0014](0014-identity-from-the-person-scope-from-the-token.md); [0040](0040-a-chunk-says-where-its-words-came-from.md)

## Problem

Ingestion retains content-addressed originals and records their SHA-256
digests, but the HTTP surface can expose derived text, chunks, and facts. A
client therefore cannot download the exact bytes that a document's digest
describes, compare those bytes to the ledger, or replay a named historical
version. The omission also turns an auditable invariant into an internal
assumption: nothing on the public boundary says whether a recorded digest can
still be served.

## Decision

Add two Viewer-level reads:

* `GET /api/v1/documents/{id}/content[?version=N]` serves one retained
  original. No query means the current version; a version number addresses a
  recorded ledger row.
* `GET /api/v1/documents/{id}/versions` returns the ledger's `version`,
  `sha256`, `size_bytes`, and `ingested_at`.

Content is addressed by document identity, not blob identity. The handler
selects and locks the document plus its ledger row, reads the immutable blob
inside that window, and only then releases the database transaction. This
closes the replacement/purge race rather than asking the client to retry a
claim that was briefly true.

The response carries the ledger MIME, actual byte length, a strong SHA-derived
`ETag` in quoted form, and an RFC 5987/6266 `Content-Disposition`. Historical
bytes reuse the document's current display metadata because the version ledger
records content identity and size, not a frozen historical display name or
MIME. This is an explicit compatibility boundary, not a claim that old uploads
carried metadata history.

Deletion is reversible and bytes remain readable. Purge is final: its
tombstone answers `410 Gone` on both routes. A ledger row whose blob is absent
is not a normal missing resource; it is an internal invariant failure and
answers `500`. This keeps a storage fault distinguishable from a bad document
ID or version.

## Access

Both routes reuse the Viewer authorization rule. They accept a web session or
a `utp_pat_` personal access token. Token KB scoping is still a separate
narrowing check; a scoped token receives the same `404` as an inaccessible
document. Source ingest tokens remain rejected as credentials.

## Limits

The route buffers within the existing upload cap. It deliberately does not add
Range requests, multipart previews, transcoding, a hash-keyed public blob
route, or a projection of bytes into derived text. Those are media-delivery
contracts and should be designed after callers rely on this byte-exact
baseline.
