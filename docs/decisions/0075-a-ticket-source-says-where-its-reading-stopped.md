# 0075 · A ticket source says in the API where its reading stopped

- **Status**: Implemented · 2026-10-28 · no migration (the column came with 0119) · open: the interface does not read it yet
- **Written**: 2026-10-28
- **Related**: [0072](0072-a-source-checkpoint-ends-at-a-timestamp-boundary.md) (what the checkpoint is and how it moves); [#1212](https://github.com/deeplethe/utopia/issues/1212) (the tickets a round used to skip); [0033](0033-rss-source-summaries-are-source-scoped.md) (the RSS summary — a different shape for a different question)

## Problem

A ticket source stops at its per-round budget all the time: 200 tickets, or ten
listing pages, whichever comes first. A round that stops there is a **success** —
it consumed a prefix, and the next round continues from where it stopped (0072).
The source list then says "synced" and says how many documents the round added.
It does not say *where* the reading stopped.

The one thing that says where is the checkpoint, and it lived on the source row
with nothing carrying it out of the API. So an operator asking "did this source
get everything?" had nothing to read: the run history says how many documents
each round added, not how far into the provider's timeline the reading got. The
answer was in a log line.

## Decisions

1. **The source API carries the checkpoint.** `Source.sync_checkpoint` and
   `SourceView.sync_checkpoint` hold the update time a ticket source has consumed
   through — the time whose whole group was read (0072). The list response carries
   it, and so do the create/update responses, so the two shapes of a source never
   disagree about the same row.

2. **It is the raw timestamp, not a state the server computes.** The RSS summary
   (0033) is a computed block: the server works out which of the five counts
   applies, so the client does not re-derive it from `kind` and `content_mode`.
   The checkpoint is not like that. Whether the source has more to give is a
   question only the provider can answer, and the server does not ask it between
   rounds — a state word would have to say "caught up" or "behind", and the server
   cannot know which. The timestamp is the fact; what it means is the reader's.

3. **NULL means there is no checkpoint.** Not zero, not the epoch, not "now".
   NULL says the source has not finished a timestamp group yet (first round), or
   that this kind does not read by timestamp at all. It doubles as the
   applicability signal: a kind that does not read by timestamp has NULL there, so
   the client does not need to know the kinds to know whether the field means
   anything.

4. **It says where the reading stopped, not when the round ran.** `last_sync_at` is
   the round's time and exists for every kind. The checkpoint is a position in the
   provider's timeline and exists only for the kinds that have one. The two answer
   different questions, and both are carried.

5. **The interface does not read it yet.** Where to put it and what to call it is a
   change to the source row, and that row is already dense — status, time, added,
   error, endpoint, schedule. A diagnostic that only appears in a tooltip is not
   obviously better than one that is in the API. This record settles the shape; the
   interface follows when someone wants it.

## Dead ends

- **A `state` word beside the timestamp** ("caught up", "behind", "partly read").
  The server cannot know it: the provider is not asked between rounds, so
  "caught up" would be a guess. A guess in a field that looks authoritative is how
  a wrong answer gets believed — the same reason the RSS summary computes what it
  can actually compute instead of guessing the rest.
- **A count of tickets still to read.** The server does not know the provider's
  total for a ticket source, and counting one would need a listing the round never
  made.
- **Putting the checkpoint in the run history instead.** The runs are a tail, not a
  ledger: they are capped and pruned. The position a source is at would disappear
  with them.
- **Only the list, and not the source responses.** Then the two shapes of a source
  disagree about the same row, and a client that read one and wrote the other
  would see the field vanish.
- **A separate endpoint for it.** One more route for one nullable column, and the
  list already carries the rest of the source's sync state.

## Open questions

- **Where the interface shows it.** The source row is dense; a second timestamp
  there needs a place and a wording, and neither is settled.
- **Whether a ticket source should say how much of the provider's window it has
  read.** That is a count the server does not have.
