# 0014 · Identity from the person, scope from the token

- **Status**: Implemented · 2026-10-16 · #180, #318, #1243 · open: tokens across workspaces
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

6. **A write token may set up a base** (added 2026-10-15, #1243). A repository mirrored into
   Utopia gets a base and a `statements` source; the push is the source key's job (0054), but
   creating the base and the source took a session, so a backend either held someone's seven-day
   JWT or a person did it by hand for each repository. Five routes now also take a personal
   token with `write` scope, acting as its owner: creating a base in a workspace, creating a
   source, reading a source's push key, importing an ontology, and deciding where a kind word
   binds. Listing a workspace's bases takes any token, so a client can find a base by name
   before it creates one. The reasons above hold unchanged. The actor is still the living person
   (`audit_events.actor_id` is theirs, as from a session); the person's role is still the
   ceiling, checked by the same `require_kb` and workspace role as before, so a token whose
   owner is not a workspace admin creates no base, and a system admin's write token acts as
   Owner in every base, as the admin does; writing must still be ticked, and a read-only token
   is refused with `token_read_only`; a token limited by `kb_ids` reaches only those bases, and
   it cannot create one, since a base that does not exist yet is in no list
   (`token_kb_out_of_scope`); revocation and expiry are judged in SQL on every request. Nothing
   here is a base-level machine token or a service account. A token creates only `statements`
   and `api` sources, both push-only: the server fetches nothing for them and the client pushes
   with the source key, so a backend mirroring code and docs as plain documents sets up its `api`
   source the same way. A `url`, `rss` or `custom` source starts the server fetching on a
   schedule, with stored credentials for `custom`, which would turn a leaked CI token into a way
   to make the server call any address; other kinds from a token are refused with `token_source_kind`, and a
   session still creates every kind. The audit rows these routes write (`source.created`,
   `alignment.kind_word_decided`, `ontology.imported`) carry the acting token's id as
   `detail.token_id`, so before revoking a leaked token an admin can see what it did (decision
   5). One extractor, `SessionOrToken`, takes either credential, and only these routes and
   document content (0052) use it; `AuthUser` still reads every bearer as a JWT, so the routes
   behind it stay session-only until someone has a reason to open one and says it here.

   Not done: rotating or deleting a source, changing a base's settings or members, and every
   other write still take a session. A token does not create workspaces or tokens.

7. **A token reads the graph** (added 2026-10-16, #1262). A service that mirrors into a base reads the
   base back to check what landed, and a session JWT is the wrong credential for a backend
   (decision 6). Four reads now also take any live token, acting as its owner: searching a base's
   entities (`GET /kbs/{id}/entities`), an entity's detail (`GET /kbs/{id}/entities/{eid}`), its
   record-axis history (`…/history`), and a fact's evidence (`GET /kbs/{id}/facts/{fid}/evidence`).
   Search finds the entity by name; the detail gives its facts, names and derivations; the
   history says when we changed our mind; the evidence ties a fact to its sentence. Read is the
   lowest scope, so any token qualifies; the owner's role is still the ceiling and Viewer
   suffices; a token whose `kb_ids` exclude the base gets 404, as for document content (0052).
   Changing an entity on the same path (`PATCH`) still takes a session.

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

- 2026-10-15: the status line still said `can_write` was hard-coded false. #318 had already
  computed it from the token's scope and the person's role, and `remember` has been reachable
  over MCP with a write token since then. The line now names what is open.
- 2026-10-16: decision 6 let a token create only `statements` sources, though an `api` source is
  push-only as well; a backend mirroring several kinds of documents pushes them through one. A
  token now creates `api` sources too.
- 2026-10-16: decision 7 opens four graph reads to any token; decision 6's "only these routes
  and document content" now also counts them.

## Open questions

- **`query_data` and `remember` over MCP.** The first version ships `search_chunks`,
  `search_docs`, `find_entities`, `entity_facts` and `changes`. `remember` waits on the gate
  of 0015; what evidence an external agent's fact carries and how SQL runs are audited are
  still unanswered.
- **Tokens across workspaces.** `kb_ids` is a base-level whitelist; a workspace-level grant
  would look much like `data_source_grants`, and the two concepts may merge then.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> implemented (#161 record, #180 code) · `personal_tokens` and a Streamable HTTP MCP server at `POST /api/v1/kbs/{kb_id}/mcp` with five read-only tools, `application/json` responses rather than SSE · the account-level "Agents & tokens" page at `/account/tokens` (0016 A2): plaintext shown once beside a per-base client config snippet, the list keeps the prefix, revocation leaves a trace · `can_write` is still hard-coded `false`, so a `write` scope changes nothing yet
