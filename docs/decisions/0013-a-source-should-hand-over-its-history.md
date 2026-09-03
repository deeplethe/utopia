# 0013 · A source hands over its history

- **Status**: implemented for `github_issues` (#134), `jira_issues` (#135) and `notion` (#213, pages keep their own clock); WebDAV shares and object storage (S3 / Azure / GCS) arrived as plain file sources (#200, #207, #209) · Feishu and Confluence not started · the `instant` precision has not been triggered
- **Written**: 2026-08-31 · condensed into English 2026-09-03
- **Related**: the bitemporal ground of [0001](0001-ontology-import-and-governance.md); the same judgment on the corpus side in `scripts/bench/fetch-wiki-history.mjs` (#122); [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) is the other end of the line — this record is about how things come in, that one about the rules they land by; the grant layer added afterwards (#142, `data_source_grants`) decides who may mount a source: provenance visible, destination governed

## The problem

The product is a bitemporal ledger, so a source is worth connecting only if it passes four
tests: real timestamps (the recorded-time axis depends on them); the ability to contradict
itself (otherwise `supersedes` has nothing to do); a stable identity so a new version of the
same thing is recognized (`external_key`); and enterprise knowledge actually living there.
Miss one and it degrades into another web scraper. Issue trackers pass all four.

The valuable part of a ticket is not "it is closed now" but "opened 08-18, closed 08-20,
assigned to whom in between, priority changed how". Syncing only the current state builds
that timeline sync by sync, and everything before the first sync is lost — while most systems
already keep the change history and only need to be asked. #122 made the same call for
Wikipedia; there revisions have to be sampled, an issue tracker hands the events over.

## Decisions

1. **One ticket is one document, and the body carries the history.** Both connectors render
   the same shape on purpose: a heading, dated declarative sentences (`Currently Closed.`,
   `Resolved on 2011-07-19.`), a `## History` list of `date — who changed Field: A → B`, then
   `## Comments`. Sentences with dates, never key-value pairs: `Opened by X on 2026-08-18`
   extracts to a fact with `valid_from`; `created_at: 2026-08-18` leaves the model to guess.

2. **GitHub fetches events per ticket.** The first version routed everything through
   repository-level endpoints to spare 200 tickets 401 requests. Real data showed the events
   leg was wrong: `issues/events` has no `since`, pages from newest backwards, and pull
   requests produce issue events too — this repository's issue events sat on page 5, and a
   PR-heavy repository pushes them past the page cap. The state history came back silently
   empty, and it is the reason the source exists. Per-ticket fetching costs N requests, N
   being the tickets written this round; accuracy over economy here.

3. **Jira's traps are all in field shape.** Timestamps are `2026-08-24T11:11:52.944+0000`,
   no colon in the offset, not RFC3339; an unparsed one drops a whole page. Incremental sync
   has no `since` and goes through JQL `updated >= "…"` in Jira's own format with quotes, and
   both mistakes surface as 400, not as an empty result. `fields` must be listed explicitly:
   omit `comment` and no comments come back; list nothing and one response runs to hundreds
   of KB. `expand=changelog` returns the field-level history in one call.

4. **Fixtures come from real responses.** Hand-written JSON only proves that the shape I
   imagined parses. Both fixtures come from publicly readable instances (`deeplethe/utopia`,
   `issues.apache.org`), trimmed to the declared fields — the trimming itself shows that
   undeclared fields do not break serde. It caught a bug the hand-written tests passed: the
   per-ticket events endpoint does not return the `issue` field (the context is in the URL),
   and the first version had it as required. A source without a public instance must be
   delivered with "not verified against a real instance" stated.

5. **Truncation is reported.** Kafka has 14506 tickets; a round caps at 500 pages. When the
   fetched count is below `total`, a warning says this round covered a slice, otherwise "sync
   complete" misleads. Same principle as #108 (partial extraction reported as complete) and
   #127 (one bad record must not sink a chunk).

6. **Timestamps are cut to the day, deliberately.** The connectors write
   `created_at.format("%Y-%m-%d")` in UTC, so `valid_from` extracted from
   "Opened by X on 2026-08-18." has day precision, and an event across UTC midnight is off by
   one day for a UTC+8 reader. Accepted because nothing is truly lost (both connectors are
   idempotent and resyncable); because `precision = day` says exactly "the day is known, the
   instant is not" — the real error would be pretending to know; and because the product does
   not yet ask "at what hour". Add an `instant` precision (CHECK constraint, extraction prompt
   and rendering branch together) only when someone needs local business days or a source
   whose events cluster around midnight.

   A related rule is already in code: world time and record time get opposite timezone
   treatment. `valid_from` / `valid_to` are calendar dates from statements in documents and
   render in UTC (local conversion shows a UTC-5 reader the previous day); `recorded_at` is a
   real instant and renders in the viewer's timezone. The difference is intentional.

## Dead ends

- **An `issues` + provider abstraction.** The second vendor was connected to find out whether
  to abstract. Not yet: the fetch strategies differ at the root (three pulls vs one call,
  `since` vs JQL, event-level vs field-level history). What they share — one ticket, one
  document, history in the body — already lives in the identical document shape and needs no
  trait; a forced interface would push the real differences into a pile of `match`.
  Reconsider when a third source lands in one of the two existing shapes.

## The same judgment elsewhere

Provenance has to be visible, and that reached the ontology after this record: a class's
shape carries its origin (#141: square = declared by a vocabulary with an IRI, circle = grown
from the corpus), and a class adopted by a vocabulary (`adopt_iri_onto_key`, #145) changes
shape — a class with an IRI drawn as a circle is a picture that lies.

## Open questions

- **Feishu / Confluence / Notion** are next: the enterprise version of the #122 Wikipedia
  history corpus — versions of one document at different times, numbered, no sampling and no
  events buried in PRs. Feishu first (`feishu_docs`): it is where Chinese users are, and its
  API hands out document versions. Three unknowns to answer first: no public instance (a test
  tenant, or "not verified" stated); rich text is a block tree (Feishu `docx` blocks, Notion
  blocks, Confluence ADF; Jira v3 has the same problem, avoided this round via v2) and needs a
  block-tree-to-text renderer; and "history" here is a version sequence, closer to #122 —
  probably sampled by how much changed rather than every version.

  The skeleton is the two existing connectors: a pure `render()` turning one record into
  Markdown (no network, so testable); `fetch_all()` for paging and auth; a `sync_*` branch in
  `ingest_sources.rs` calling `ingest_item()`, which owns identity, sha256 dedup and version
  records; and the `sources::KINDS` whitelist plus three frontend places (`SourceView["kind"]`
  in `api.ts`, the create dialog in `Library.tsx`, icons and `SYNCING_KINDS` in
  `SourcesRail.tsx`). The fourth step is the one that gets missed: the kind is selectable and
  creation fails with `kind must be one of…`, invisible to unit tests and tsc (#134 hit it).
  Credentials go in and never out — an empty field on edit keeps the stored value, as in
  `sync_custom`. Acceptance: unit tests on `render()`, fixture tests where a real response is
  obtainable, create → sync → versions present → second sync adds 0, and clean
  `cargo clippy --workspace --all-targets` and `npm run typecheck`.

## Revisions

- 2026-09-03: every connector's credentials stay on the server (#246). Until now only `auth_header` was stripped from responses; the object-storage, WebDAV and Notion keys went out to every viewer. The keys now live in one list, `SOURCE_SECRET_KEYS`, shared by the listing, the create / update responses and the update merge (blank or missing keeps the stored value, an explicit `null` removes it). Adding a connector means adding its keys there first.
