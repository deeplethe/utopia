# A source checkpoint ends at a timestamp boundary

In progress 2026-10-25 (#1359, migration 0119); open: custom batch progression and recovery (#1397).

## Context

Source syncs can stop after 200 items and still record success. The next run
uses that successful run's start time as its lower bound, so it can skip items
that were never imported. [Issue #1212](https://github.com/deeplethe/utopia/issues/1212)
reproduces this with a custom response containing 201 items.

A checkpoint must describe the data consumed. For GitHub and Jira, an item's
source update time provides that position, but a timestamp cannot identify a
position inside a group of items that share the same time.

The [reader probes](https://github.com/deeplethe/utopia/issues/1212#issuecomment-6183423449)
show why that distinction matters. With one shared update time, GitHub's
1,001 items span eleven 100-item pages; Jira's 501 items span eleven 50-item
pages. A fixed ten-page limit repeatedly returns the same prefix. Keeping the
timestamp repeats that prefix; advancing past it loses the unread tail.

## Decision

Use a timestamp watermark: the latest source update time whose entire group
has been consumed. **A GitHub or Jira run stops only at a timestamp boundary.**
This follows the [maintainer's decision](https://github.com/deeplethe/utopia/issues/1212#issuecomment-6261482074).

### GitHub and Jira

GitHub/Jira store their watermark in `sources.sync_checkpoint`, initially NULL even
when successful audit rows exist. Progress and the successful run commit together.
The 2026-10-23 revision below records the listing verification, complete GitHub
discussion reads and bounded waits used by these readers.

Read items in ascending update-time order. Jira breaks ties by key so an unchanged
result keeps the same page boundaries. Skip items at or before the checkpoint
before counting either budget. When the 200-item processing budget
or ten-page listing budget is exhausted inside a group, continue reading and
processing until that group is complete. Stop before consuming the next
timestamp group, then save the completed group's time as the checkpoint.
Reading ahead to identify the next group does not make that group consumed.

For example, if item 200 belongs to a group ending at item 240, consume all
240 items and leave the next group for the next run. The 1,001-item GitHub
and 501-item Jira groups above must each arrive in full on the first run.

The next run consumes only items updated strictly after the checkpoint.
GitHub's `since` is inclusive in practice, and Jira's `updated >=` bound is
rounded to the minute, so either provider can return items already consumed.
Skip every returned item at or before the checkpoint; these items count against
neither the 200-item processing budget nor the ten-page listing budget.
Pages containing only skipped items therefore cannot exhaust the listing
budget. For GitHub, `since` can be the checkpoint plus one second.

Store the completed timestamp on the source, independently of retained sync
audit rows. Do not derive it from a run's start or finish time, or advance it
to the clock time merely because a query is empty. A failed run leaves the
previous checkpoint intact. Retry already committed items through the existing
document, version and processing-job deduplication.

There is no persisted page, offset, fetched response or continuation state.
An unusually large timestamp group makes one run longer. Do not add a separate
group cap or a group-size alert; use the existing rate-limit backoff. The
exception applies to source listing and processing, not permission to replace
a GitHub document with a truncated comments or events history.

### Revision 2026-10-23: verification, complete discussions and bounded waits

Keep the second listing pass before ingestion. Re-read through the selected
timestamp boundary and compare the listed identities and update times with the
first pass, including skipped old rows and excluded GitHub PRs. Those rows can
still move offsets. For example, after reading issues 1–100, updating issue 1
can move it to the tail; the next page then starts at issue 102 and silently
misses issue 101. Advancing the checkpoint without detecting that movement can
make issue 101 unreachable on later incremental runs. Jira has the same risk.

The second pass roughly doubles requests for the selected listing prefix, not
GitHub's per-ticket comments and events requests. It does not extend the chosen
window, and changes only beyond that boundary do not invalidate the prefix.
Comparison ignores ordering within one timestamp group. A changed identity or
update time fails the run before ingestion, preserves the previous checkpoint,
sets `last_sync_status` to `failed` and raises the existing `source.sync_failed`
alert. A later run starts at the same checkpoint. Keep this failure behavior;
do not add repeated scans inside the same run. This detects observed movement,
not an atomic snapshot. A continuously changing prefix can repeatedly fail;
progress under sustained concurrent updates is not guaranteed.

GitHub comments and events must be read to the end, even beyond ten pages.
The listing budget is not a limit on a selected ticket's discussion history.
Do not skip a large discussion, return truncated content or advance progress
past a ticket whose details could not be read completely. A detail-page failure
keeps the old document and checkpoint. Large discussions therefore increase
requests, memory use and run duration. Jira's search `total` remains required:
without it, a premature empty page could be mistaken for a complete group.

For explicit rate limits, keep at most three retries of the current request,
preserving its in-memory position. Honor a valid wait hint only when it is at
most 30 seconds per retry, retaining the existing minimum backoff. An invalid
or longer hint fails the run immediately without sleeping or shortening the
provider's requested wait. This prevents one long wait from occupying a sync
slot for hours; it is not a total run-duration limit. The existing job retry
and source schedule govern later attempts and do not persist `Retry-After`, so
they do not guarantee that a later attempt waits until the provider's deadline.
Strict cross-job rate-limit scheduling remains outside this revision.

### Custom

The endpoint already returns the whole requested response. Walk its items in
order, charging only `Created` and `Updated` outcomes against the 200-item
budget. `Unchanged` items do not consume that budget. Mark the batch incomplete
only when processing stops with items left, and retain the previous checkpoint.
Retries can then pass the unchanged prefix and reach the remaining items.

Once the whole response, including its deletions, is processed successfully,
advance the checkpoint to that run's start time, preserving the existing
protection for changes during the run. Keep the committed counts visible for
incomplete runs. No source-update field, pagination token or saved response is
added to the custom protocol; `doc_time` remains document time.

The custom fix is being reviewed in
[PR #1397](https://github.com/deeplethe/utopia/pull/1397). It does not resolve
the GitHub and Jira parts of #1212.

## Recovery and tradeoffs

Old successful runs cannot prove that their windows were complete. On upgrade,
clear the checkpoint once for these three source kinds and recover from the
beginning using the same rules. Preserve documents, versions, jobs and audit
history; deduplication keeps unchanged content from creating new versions.
Resume incremental reads afterward.

This design trades a hard per-run bound for progress through a timestamp group.
It does not make a provider's paginated result an atomic snapshot. Concurrent
updates and failures still need coverage before the implementation can claim
that checkpoints never cross unread data.

## Rejected approaches

- **Keep the old checkpoint but always take the first 200 items.** Retries
  never reach the tail. This is why custom must count changed items only.
- **Use a timestamp with an unconditional ten-page limit.** The equal-time
  examples above still stall. Completing the group removes that limitation.
- **Persist continuation state or a full response snapshot.** The original
  proposal stored pages and consumption positions in `sources.sync_state`.
  Finishing each timestamp group avoids that storage and recovery machinery.
- **Resume from a saved offset, even in creation order.** Deletions, transfers
  and permission changes can move page boundaries. Keeping boundary identities
  and restarting scans adds complexity the chosen design does not need.
- **Advance to the run's start time after partial work, or use custom
  `doc_time`.** Neither proves that the remaining source changes were consumed.

## Acceptance

Reuse the real readers and existing HTTP/database fixtures. The implementation
must demonstrate:

- GitHub's 1,001-item and Jira's 501-item equal-time groups are fully imported
  in the first run, beyond ten pages.
- Exhausting either budget completes the current group but leaves the next
  group for the next run, with the correct persisted checkpoint and queries.
- After importing the 1,001-item group at time T, the next run skips any
  returned items at or before T without charging either budget, imports a
  later group and advances the checkpoint past T.
- GitHub's inclusive boundary and Jira's minute-rounded query bound preserve
  this progress without omissions or duplicate versions and processing jobs.
- A failed page fetch, detail fetch or write cannot advance the checkpoint;
  retry recovers the whole group. Updates during pagination remain reachable.
- A page-moving update rejects the window; once the listing stabilizes, a
  retry recovers the skipped issue and a later run reads the moved issue.
- GitHub comments and events beyond ten pages are fully read; failure on a
  later detail page preserves the old document and checkpoint.
- A 30-second rate-limit hint is accepted; a 31-second hint returns an error
  without sleeping or retrying that request. Cover seconds and HTTP-date hints.
- Custom's 201 changed items finish on the second run; 450 finish over three
  capped runs. Cover an unchanged prefix and no custom update field.
- The one-time recovery scan preserves existing data and returns to incremental
  sync. GitHub replacements retain complete discussion content.

Database assertions must run with PostgreSQL/pgvector and
`UTOPIA_TEST_REQUIRE_DB=1`.
