# 0014 · Identity from the person, scope from the token

- **Status**: implemented (#161 record, #180 code) · `personal_tokens` and a Streamable HTTP MCP server at `POST /api/v1/kbs/{kb_id}/mcp` with five read-only tools, `application/json` responses rather than SSE · the account-level "Agents & tokens" page at `/account/tokens` (0016 A2): plaintext shown once beside a per-base client config snippet, the list keeps the prefix, revocation leaves a trace · `can_write` is still hard-coded `false`, so a `write` scope changes nothing yet
- **Written**: 2026-09-01 · condensed into English 2026-09-03
- **Related**: migration `0014_data_source_grants` gave data sources a grant layer; this is the same question where a machine knocks. [0004](0004-language-and-localization.md) has the server speak English only, MCP error codes included. [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) removes the main objection to opening `remember` over MCP.

## The problem

Exposing Utopia's tools over MCP raises one question before transport: as whom does the
client connect? Neither existing credential fits. The JWT follows a person but is stateless,
expires in seven days and cannot be revoked — a client configured in a file on someone else's
machine would be reconfigured weekly, and a lost laptop waits for expiry.
`sources.ingest_token` follows a source, is stored in plaintext, never expires and can only
push documents in.

## Decisions

1. **The token acts as the person.** Effective permission = the person's role ∩ the token's
   scope. A token can only narrow, never widen: a viewer's token ticked `write` stays
   read-only. Existing guards (`require_kb(kb_id, Role::Viewer)`, `access::kb_role`) still
   receive a `User`; the audit trail shows a person; deactivating the person kills their
   tokens; several bases need no extra keys.

2. **Scope still narrows separately, because of the confused deputy.** An MCP client is
   someone else's agent under someone else's system prompt, reading untrusted content from the
   base — a document saying "run this SQL" or "remember X" may be obeyed with the person's
   full permissions, and Utopia knows nothing about that client's prompt or its other servers.
   Full permission means read-only SQL against every production database mounted in every base
   the person can reach (`query_data`) and writes to the ledger (`remember`), all hanging off
   a string in `claude_desktop_config.json`. So the default token is read-only and limited to
   one base; writing must be ticked.

3. **This token is hashed while `ingest_token` is not.** `0002_ingest.sql` reasoned that once
   the database is lost the documents are lost anyway, so hashing buys nothing — true for a
   token that can only push documents in. A personal token reads production databases outside
   Utopia through `query_data`, and that warehouse should not fall with Utopia's database.
   Different blast radius, different storage. SHA-256 rather than argon2: a high-entropy
   string does not fear brute force, and a per-row salt would defeat the `UNIQUE` index on
   `token_hash`.

4. **Shape.** `personal_tokens(id, user_id, name, token_prefix, token_hash, scope, kb_ids,
   expires_at, last_used_at, revoked_at, created_at)`: `scope` defaults to `read`; `kb_ids`
   NULL means every base the person can reach; `expires_at` NULL means never, but the UI
   defaults to 90 days and "never" is an explicit choice; `last_used_at` answers "is this
   still in use" before revoking. The prefix `utp_pat_…` against ingest's `utp_` tells the
   kinds apart in logs and config files. `user_id` cascades on delete, unlike the bare foreign
   key on `audit_events.actor_id`: the ledger must outlive the user, the keys must not.
   Revocation writes `revoked_at` rather than deleting the row: the revocation is a trace.

5. **Every tool call checks scope.** The lesson of `0014_data_source_grants`: list filtering
   only guards what is visible, the mount endpoint is called by id, so the guard sits on both
   sides. Rather than a handshake check followed by trust for the connection's lifetime, every
   POST re-runs authentication (revocation and expiry in the SQL `WHERE`), `covers()` and
   `require_kb`; a token revoked mid-session fails on its next call. Each call writes an audit
   row (`mcp.tool_called`, target = the token), so attribution is real.

## Dead ends

- **Base-level machine tokens**, one per base and unrelated to a person. It would be a third
  authorization model beside workspace membership and base roles — "what can this agent see"
  needs three tables, and a mistake in any fails towards "gave too much". And attribution
  becomes fake: `audit_events.actor_id` records living people with an `actor_label` snapshot,
  while a machine token's facts would belong to a synthetic id.

## Revisions

- 2026-09-02: an unstated precondition — #175 moved the seven tools out of the `match` in
  `chat.rs` into `tools.rs`, so chat and MCP share one implementation. The JSON schemas stay
  in `chat.rs`; `search_chunks` still says results can be cited as `[n]`, which MCP clients
  cannot see.
- 2026-09-02: the placeholder crate `utopia-mcp` advertised three tools that never existed;
  the server lives in `utopia-server/src/api/mcp.rs`. Deleted with `utopia-graph` and
  `utopia-connectors` ([0016](0016-close-the-open-seams-before-cutting-new-ones.md) A2).

## Open questions

- **`query_data` and `remember` over MCP.** The first version ships `search_chunks`,
  `search_docs`, `find_entities`, `entity_facts` and `changes`. `remember` waits on the gate
  of 0015; what evidence an external agent's fact carries and how SQL runs are audited are
  still unanswered.
- **Tokens across workspaces.** `kb_ids` is a base-level whitelist; a workspace-level grant
  would look much like `data_source_grants`, and the two concepts may merge then.
