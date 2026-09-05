# 0023 · RSS observations are not documents

- **Status**: Implemented in #326
- **Written**: 2026-09-05
- **Discussion**: [maintainer feedback on #326](https://github.com/deeplethe/utopia/pull/326#issuecomment-5547020427). This record follows implementation; it does not claim the issue/record-before-code sequence in `CONTRIBUTING.md` was followed.

## Why retain an observation

A feed can drop an item before a queued acquisition runs. Documents cannot retain an item that has not produced acceptable content, and creating a document from its summary would misrepresent full-content ingestion. The first successful feed response also needs durable identities so enabling full-content mode does not backfill existing items. An empty successful response establishes that baseline too; a failed response does not.

Entries without a publisher-provided GUID/Atom ID and without a usable HTTP(S) article link are silently skipped in both feed-content and full-content modes. Titles are not an identity fallback. The shared ingestion filter drops these entries before either mode processes them, so they create neither documents nor observation/diagnostic rows.

## Decisions

Use one RSS table with `baseline`, `candidate`, and `no_source` classifications. Retain bounded acquisition inputs and the current-job relationship, not another execution lifecycle. Jobs own attempts, retry timing and operational errors; documents own identity, accepted content and versions. Successful publication completes the current job transactionally and clears observation body inputs.

Keep activation generation and baseline time on `sources`. Capture configuration, generation and database time before fetching. Recheck generation under the source lock before discovery or publication. Generic retry admission takes the same source lock and respects the same 25-job capacity.

Retain external identity in existing document-deletion history when purge releases the live document key. Compare deletion/purge time with the job's immutable observation time. Old backlog must not resurrect purged content; a genuine later observation may authorize replacement. A completed-job fence alone failed because an observation can wait for admission without any job. A nullable document reference also misses documents created after observation. The chosen fence uses serialized database wall-clock timestamps; clock-step safety is not established.

Use one Readability/Markdown page extractor for generic HTML and linked RSS pages. Feed fragments bypass Readability because they are already entry-scoped. Fetch policy stays outside the parser.

## Verification and limits

Required-database tests cover migration replay, purge/reappearance, concurrent publication/deletion and retry capacity. HTTP-backed tests cover a 201-item baseline, empty/failed feeds, stale responses and feed-native hydration through processing-job creation. This does not establish deployment readiness or downstream model-processing completion. Internal entry state and source counts share a projection; per-entry troubleshooting data is not exposed through a public RSS endpoint.
