# 0005 · The alert center

- **Status**: Built · five kinds live (`source.sync_failed`, `llm.unreachable`, `llm.rate_limited` #160, `llm.out_of_credit` #182, `data_source.schema_sync_failed`); the panel has search and per-group paging; decisions 1 and 2 were overturned during implementation; `document.no_text_layer` still unwired.
- **Written**: 2026-08-29 · condensed into English 2026-09-03
- **Related**: adjacent to the Review queue ([0001](0001-ontology-import-and-governance.md) P4); [0004](0004-language-and-localization.md) is why titles are assembled on the client.

## The problem

Failure state was scattered over six places with no single view: `jobs.status='failed'` + `jobs.last_error` (no UI at all), `documents.status='failed'`, `documents.graph_status='failed'`, `sources.last_sync_status='failed'`, `source_sync_runs.status='failed'`, and the log. It had been patched once, by adding `graph_error` to `documents`: one column per failure class. With reasoning, execution, OCR and lakehouse connections still to come, that road ends with a dozen error columns and no answer to "what is wrong right now".

The real harm is silent failure. Drop in 100 PDFs, 12 of them scans, and the UI shows 100 green rows; the user finds out when an answer lacks that contract, and never suspects ingestion.

**Boundary with Review.** Review holds what needs a decision (merge these two entities?); the alert center holds what a person needs to know (this document did not get in). Review guards the correctness of knowledge; alerts guard the health of the system. `extraction_drops` (11 drop reasons, shown per document on the library row) is a third channel: something in this document did not land. None of it goes to Review.

## Decisions

1. **Aggregation belongs to the view.** 100 PDFs with 12 scans must read as one line, "12 documents have no text layer", or the alert center is ignored within two weeks. But a stored aggregate is live and must be cleared when things recover, or it lies. So: one row per failure, never updated; the read side folds adjacent rows of the same `(kb, kind)` in SQL (gaps and islands, two `row_number()` subtractions), because paging is by group and a client-side fold would split a run at a page boundary. Counts are computed and never stale; a different failure in between is a different episode.
2. **"Is it fixed?" is a question the alert center does not answer.** Every failure is a new row. Once the fault is fixed there are no new rows; read ones sink and the badge goes dark; a recurrence two months later is a new alert. No success signal, no probe, no time constant. The cost is rows (a broken hourly source writes 24 a day), purged after `alerting::RETAIN_DAYS` (30). Whether something is still broken is on the source page and in document status; an alert's job is to make someone look.
3. **Read state is per person.** `alert_reads` is a two-column table; unread means "visible to me and not clicked by me". Reading says whether I have seen it, nothing about whether it is over. Nobody can read an alert away for someone else, as with GitHub notifications and Slack unread.
4. **Visibility reuses the KB role chain.** `kb_id IS NULL` marks a system-level alert (the three LLM kinds), visible to `users.is_admin` only; a KB-level alert is visible to roles ≥ `min_role` in that KB, and admins see everything, as in `access::kb_role()`. `min_role` lives on the row: configuration alerts go to admins, content alerts (parse, extraction, source sync) to editors and above, because whoever uploaded the 12 scans needs to know more than the admin does.
5. **A row names one subject and carries no display text.** `subject_id` is a single column, and `detail` keeps the subject's name so the alert renders after the subject is deleted. Titles are assembled on the client by kind ([0004](0004-language-and-localization.md)), so search matches KB name, `detail` and the kind code, never the title.
6. **Push is one global stream with no data and no permission check.** The per-KB SSE route cannot carry a cross-KB badge, so `/alerts/events` rides the existing `AppEvent` broadcast (kind `alert`) and only tells clients to refetch; the list query decides visibility once. Someone without permission is woken for nothing, and the push path holds no permission logic.
7. **Classification is a pure function on error types.** `alert_for` decides the kind from the error's type, never its text, and has unit tests; job failures reach it through `observe_job_failure` only once retries are exhausted. `llm.unreachable` means "no parseable answer from the endpoint": connection refused, or a proxy answering HTML. A clean 4xx is the API saying the key or quota is wrong, another person's problem (`rate_limited`, `out_of_credit`).
8. **A bell with a popover**, because a page pulls people away from their work and then nobody goes. The badge is a red dot ("something unseen" is binary; a count jumps with every retry); a click marks read, hovering does not.

## Dead ends

- **Stored aggregation**: `(kb_id, kind)` unique among unresolved rows, repeats appended to `subject_ids UUID[]`. Built first; three bugs shared one root: the row was hollowed out as things self-healed (empty `subject_ids` and `detail`) and produced titles like "0 sources failed to sync". Its `WHERE resolved_at IS NULL` partial unique index would also have failed silently for `kb_id IS NULL`, since NULL never collides; the same trap is why `mark_group_read` compares `kb_id` with `IS NOT DISTINCT FROM`.
- **Self-healing** (`resolved_at` cleared by the producer). Every new kind must implement its own "what counts as fixed": `source.sync_failed` gets it free from `finish_sync`; `llm.unreachable` needed a background probe hitting the endpoint every minute, guarded by "is the alert still lit?". A missing clear is invisible at compile time and shows as an alert that never goes out. Recorded because the idea is tempting enough to be proposed again.
- **Dedup by `(kb, kind, subject)` with a bumped timestamp**: bounded rows, no retention, rejected on paper. Recurrence and "never fixed" become indistinguishable, so either read-once-forever (a recurrence is silent) or every bump re-unreads (a known fault lights up hourly). Missed alerts cost more than rows.
- **Shared read state**: any admin opening an alert marks it read for all. A glances in the morning and puts it off; B and C never learn it happened, and no trace shows the gap.
- **Reusing `audit_events`**: the ledger records what people did; alerts record what the system did, with different retention (30 days versus never), visibility and readers.
- **A UNION over the six status columns, no table**: zero migrations, but no per-user reads and no history; once the source recovers, "why did last Wednesday's sync fail" is gone.
- **An empty framework first**: migrations are the hardest thing to take back, and aggregation, self-healing and per-user reads can only be validated with data flowing. Two real sources went first (`source.sync_failed`, and `llm.unreachable` for `kb_id IS NULL`), and two of the three original decisions failed on them before any tag was cut.

## Revisions

- The "resolved for everyone at once" half of decision 3 went with decision 2; the per-person half stands.
- 2026-09-02: "no new permission logic" did not hold. The list filters in one SQL statement through `access::visible_kb_roles()`, a `VISIBLE` CASE and a `rank()` in `alerts.rs` that must stay in the same order as `Role`'s `PartialOrd`: three places to edit when roles change, exactly the hidden rule this repository fears.
- 2026-09-02: three more kinds were wired from existing error classes with no new detection logic. `data_source.schema_sync_failed` is raised from a state left behind (source attached, schema never ingested), so "any execution path reports on failure" was too narrow.
- `llm.unreachable` first matched only transport failures, so the commonest fault (a wrong URL, a proxy answering HTML) produced no alert at all.

## Open questions

- **`document.no_text_layer`** waits for detection (an empty parse result) and an OCR endpoint, so the message can say "no text layer; configure OCR and reprocess". When wired, do not ask when it is fixed: one row per scan, the panel folds adjacent rows, reprocessing needs no cleanup.
