# 0063 · Stopping a chat ends the generation

- **Status**: Implemented 2026-09-27 (#969, migration 0095)
- **Written**: 2026-09-26 (conventions in the [README](README.md))
- **Related**: [#934](https://github.com/deeplethe/utopia/issues/934), [0042](0042-the-chat-loop-is-a-runner-with-hooks.md)

## Problem

Stop currently closes the browser stream while the server continues generating and saving
the answer. A follow-up can start before the first answer finishes, interleaving messages
and building its context without that answer. Refresh must still allow generation to continue.

## Decision

- **Stop is explicit.** The authenticated cancel route checks ownership and signals the active
  generation. Losing an SSE connection does not cancel it. The runner checks cancellation before
  model and tool calls and interrupts an in-flight call when stopped.
- **Keep the question and published partial answer.** Save one assistant message with
  `stopped = true`, including when its content is empty. Keep the steps, sources and complete
  tool exchanges already obtained. Do not save an unvalidated, buffered final-answer candidate.
  History displays the stopped flag; model context explicitly identifies the answer as incomplete.
- **One generation owns the conversation until it finishes saving.** Reserve it before appending
  the question or reading context. Another submission or retry receives HTTP 409 with `answer_running`;
  requests are neither queued nor used to replace the active generation.
- **One owner saves and ends the turn.** The cancel route does not save a second answer or emit
  a competing terminal. The producer saves once, then emits `done` with `stopped: true` for a
  successful cancellation. A persistence failure remains `error`. Once normal completion has
  entered its save, it wins over a late cancellation.
- **Acknowledgement is not completion.** The browser sends the cancel request and keeps waiting
  for the terminal outcome. It enables follow-up only after stopping has settled. Reattachment
  observes the same outcome; stale cancellation must not stop a later generation.

This uses the existing registry, message table and SSE terminal contract. It adds a boolean
column, not a separate job table. Cancellation stops local work and closes pending calls;
it does not reverse completed tool side effects or guarantee an upstream billing adjustment.

## Verification

Cover stop before output and during generation, partial and empty stopped histories, immediate
follow-up, concurrent submissions, disconnect and reattach, duplicate and stale cancellation,
completion races, ownership checks, and failed persistence. Each turn has one persisted outcome
and at most one terminal event; a failed save cannot report successful completion.
