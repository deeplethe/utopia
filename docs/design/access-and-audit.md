# Identity comes from the person and the ledger outlives them

Records: [0014] (tokens and MCP scope), [0020] (the export and the read contract), [0026] (why on
every decision), [0025] d1 (only people write precedent), [0005] d3 and d4 (visibility), [0013]
(credentials), [0034] d5 and d6 (roles for actions), [0016] E.

## What it does today

**People, workspaces, bases, roles.** A person has an account; a workspace holds bases and grants of
data sources; a base has the roles Viewer, Editor and Admin (`access::kb_role`, `require_kb`), and
`users.is_admin` sees everything [0005, 0014]. Visibility of alerts reuses the role chain
(`min_role` on the row; system alerts to admins only) [0005].

**A personal token acts as the person and can only narrow.** `personal_tokens` (prefix `utp_pat_`,
SHA-256 hash, `scope` read or write, `kb_ids` or every base, `expires_at` with 90 days as the UI
default, `last_used_at`, `revoked_at` as a trace); effective permission is the person's role
intersected with the token's scope; every MCP call re-runs authentication, scope and role in SQL and
writes an audit row `mcp.tool_called`; the tokens page shows the plaintext once beside a client
snippet [0014]. `ingest_token` on a source is a different kind: plaintext, push-only [0014].

**The audit ledger.** `audit_events` records what people did, with an actor and an `actor_label`
snapshot, kept forever, never a notification: entity retypes and renames with before and after,
review decisions with both names and the `why` a person wrote, merges and reverts, nods, mapping
decisions, tool calls, `kb.exported`; a machine's decision carries no actor and keeps the machine's
own `why` [0001, 0005, 0025, 0026]. Precedents are read from this ledger and only rows with an actor
count [0025].

**Credentials.** Source keys, warehouse connection strings and model keys are sealed at rest
(`secrets::seal`), listed in `SOURCE_SECRET_KEYS`, stripped from every response, kept when a field
is blank on edit [0013, 0034]. A token's `user_id` cascades on delete; `audit_events.actor_id` does
not, so the ledger outlives the person and the keys do not [0014].

**An auditor reads the base without us.** `GET /kbs/{id}/export?format=turtle|jsonld` streams the
base as RDF: imported classes keep their IRI, everything else is minted under a stable URN namespace
(`?base=` overrides), every fact is an `rdf:Statement` with both clocks, confidence, evidence with
origin, qualifiers and PROV-O lineage to documents and rules; a fact held and valid now is also a
plain triple [0020, 0037, 0040]. The RDF mapping is the supported external read contract; a breaking
change needs a record; MCP `structuredContent` shares the ledger UUIDs, and a documented field is
removed or renamed only with a record [0020 revised].

## Why

- **Neither the JWT nor the ingest token fits an agent**: one expires weekly and cannot be revoked,
  the other follows a source and never expires [0014].
- **The confused deputy**: an MCP client runs under someone else's prompt over untrusted content, so
  the default token is read-only on one base and writing is ticked [0014].
- **Different blast radius, different storage**: a personal token reaches warehouses outside Utopia,
  so it is hashed while the ingest token is not [0014].
- **Base-level machine tokens were refused** because attribution would become synthetic and a third
  authorisation model fails towards giving too much [0014].
- **Every call checks scope** because list filtering guards only what is visible [0014].
- **The ledger records what people did; alerts record what the system did**, with different
  retention and readers [0005].
- **Reification and standard terms** so the export opens in what the reader already runs, and
  `prov:invalidatedAtTime` means what the record axis means [0020].
- **A URN, not the Host header**, so two exports a year apart line up [0020].

## Proposed and not built

- OIDC SSO, backup and restore [0016 E, #712]. 0016 E also listed credentials encrypted at rest as
  pending; 0034, written later, records `secrets::seal` as already holding model keys, connection
  strings and source credentials, and the later record holds.
- A SPARQL endpoint over an in-memory projection; import of our own export; a download button
  [0020].
- Conflict and review state, chunk identity behind a quote, and historical proofs in the export
  [0020].
- Tokens across workspaces, possibly merging with `data_source_grants` [0014].
- A request queue for action grants [0034]; a theme and language that follow the account [0038].

## Open questions

- What evidence an external agent's fact carries and how its SQL runs are audited [0014].
- Whether 0014's status line still holds: it says `can_write` is hard-coded false, while #735
  exercises `remember` over MCP with a write token; check the code before relying on either.
