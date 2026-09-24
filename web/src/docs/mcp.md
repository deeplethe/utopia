# Agents over MCP

Utopia serves every knowledge base as a **Model Context Protocol** server. Any MCP client — Claude Desktop, Cursor, an agent framework, a script — can search a base, read a document, look up an entity and ask what changed, with the same permissions as the person whose token it carries. Reading needs nothing but a token. One tool records: `remember` asks a `write` token held by an editor, and what it records **waits for a person's nod** before it reaches the graph.

## Get a token

Tokens belong to people, not to bases: **Account → Agents & tokens → Personal access tokens**.

| Field | Meaning |
|---|---|
| Name | For your own bookkeeping; shows in the token list and in the audit log |
| Scope | `read` (default) or `write`. Scope is a **ceiling**, not a grant: a token can never do more than its owner can. `remember` needs both — the `write` scope, and editor rights in that base. A viewer's `write` token still cannot record |
| Knowledge bases | Optional. Leave empty and the token reaches every base its owner can open; pick some to narrow it |
| Expires in | 90 days by default |

The token value (`utp_pat_…`) is shown **once**, when it is issued. Revoking it takes effect on the next call.

Anything an agent does with the token is recorded in the base's Activity as the token's owner, with the tool name, so a token is never a way around the audit trail.

## The endpoint

```
POST /api/v1/kbs/{kb_id}/mcp
Authorization: Bearer utp_pat_…
Content-Type: application/json
```

One endpoint per knowledge base, speaking JSON-RPC 2.0 (MCP protocol version `2025-06-18`, stateless HTTP). The `kb_id` is in the base's URL in the browser: `/kb/{kb_id}/…`.

Three methods are served:

- `initialize` — capabilities and protocol version
- `tools/list` — the tools below, with their JSON schemas
- `tools/call` — run one

## The tools

| Tool | What it answers |
|---|---|
| `search_chunks` | Full-text + semantic search over the base's documents. Returns the six best-matching passages, each cut at 800 characters and carrying its `document_id`. Pass `as_of` to search the base as it stood at that moment — earlier versions, documents deleted since; full-text recall stays current, so hits are right but may be incomplete |
| `get_document` | The full text of one document, all sections in order, by `document_id`. Use it when a search hit is the right document but the excerpt does not carry the answer. Capped at 24,000 characters, and says so when it cuts |
| `find_entities` | Entities by (partial) name: id, type, and a disambiguator when several share a name |
| `entity_facts` | One entity's facts with validity ranges. Pass `at` (a date) to see the world as of that day; this is the tool for "who was X in 2024". Pass `as_of` (a date or an RFC3339 moment) to see the facts **as the base held them then**, before later corrections, retractions and merges — "what did we have on record before the memo arrived". The two combine: `at` for the date asked about, `as_of` for when |
| `neighbors` | The entities linked to one entity, one hop, grouped by predicate; narrow with `predicate` or `object_type`. Takes a name as well as an id |
| `timeline` | One entity's dated facts in world-time order; `since` / `until` narrow the window |
| `paths_between` | The chains of facts joining two entities, up to three hops, shortest first. With `at`, every edge must hold at that moment; with `as_of`, the chains as the base held them then |
| `changes` | What the graph learned or revised in a window of **record** time: asserted, corrected, rejected, merged. Needs no entity; use it when the question names a period, not a subject |
| `search_docs` | Utopia's own manual, for questions about how the platform works. Never the user's documents |
| `remember` | Record one sentence into the base's memory. **Needs a `write` token held by an editor**; a token without it does not see this tool in `tools/list`, and calling it anyway says why |

The two time axes matter here. `at` reads **world time** (when something was true); `as_of` reads **record time** (what Utopia held at that moment, before it revised it), and `changes` lists what moved on that axis in a window. They are separate parameters on purpose: folded into one they would answer "what happened in March" with "what we learned in March", and both look plausible.

## The external read contract

**RDF export is the supported machine-readable read contract.**
`GET /api/v1/kbs/{kb_id}/export?format=turtle|jsonld` streams the whole base,
including retracted and corrected assertions, both time axes, quoted evidence,
source documents, and derivations linked to their rules and premise statements.
It uses RDF reification, PROV-O and schema.org. This mapping is the compatibility
boundary: a breaking change needs a decision record explaining why.

Entities, assertions, derivations, rules and documents have UUID-based IRIs:
`urn:utopia:kb:{kb_id}:entity:{id}`, `…:fact:{id}`, `…:derived:{id}`,
`…:rule:{id}` and `…:document:{id}`. The same stored object keeps its identity across exports;
rebuilding a graph is not an identity-preserving operation. `?base=https://example.org/`
instead mints `https://example.org/kb/{kb_id}/{kind}/{id}`. Keep the same base when
joining exports. Imported classes and relations retain their original IRIs.

Ontology export includes explicitly stored `owl:inverseOf` and
`rdfs:subPropertyOf` links, using the target property's imported IRI or existing
key-based IRI. It copies declarations within the base; it does not add reciprocal
links or compute a transitive closure.

A derivation links to `…:rule:{id}` through `prov:wasGeneratedBy`. The rule
resource is typed `prov:Activity` and carries an `rdfs:label`: the business
rule's name, or the axiom rule's kind. The label is human-readable text, not a
machine-readable rule definition; it does not reliably identify the rule family.
Rule criteria, operands and expressions are not currently exported. Their RDF
representation is under discussion in [#902](https://github.com/deeplethe/utopia/issues/902).
A rule IRI identifies the stored rule, not a historical version of its definition.

MCP remains the agent-facing surface. The structured results below use the same
UUIDs, so an integration can join a selected result to the exported ledger.
Ordinary `/api/v1` UI response shapes are **not** a compatibility promise: they
have no OpenAPI contract or deprecation policy. The export route above is the
explicit exception, not a promise covering every route with that prefix.

The export does not yet include conflict/review state ([#564](https://github.com/deeplethe/utopia/issues/564))
or the chunk identity behind a quote. Per-entity export and a SPARQL endpoint are
also not implemented. Neither MCP nor the export promises historical proof
snapshots: `as_of` selects the derivations held then, but proofs use the current
premise links. Source-document links are the stored provenance, not a separately
versioned snapshot of the evidence set.

## Structured results alongside the text

`find_entities`, `search_chunks`, `entity_facts` and `changes` return a JSON object
in `result.structuredContent`, beside the unchanged human-readable `content`.
No extra argument or protocol upgrade is needed. Other tools remain text-only.
Read the JSON field directly; the text is presentation, not a format to parse.

Every structured result includes `kb_id`. IDs are opaque UUID strings, timestamps
are RFC3339 with fractional seconds preserved, unknown optional values are `null`,
and empty collections are `[]`. Clients should tolerate additional fields.
Removing or renaming a documented `structuredContent` field is a breaking change
and requires a decision record explaining why.
Failures have `isError: true` and no structured success payload; an empty result
is a successful read, not an error.

| Tool | Structured fields |
|---|---|
| `find_entities` | `entities[]`, in the same ranked order as the text: `id`, `name`, `type_key`, `type_label`, `disambiguator`, `fact_count`. This is a candidate list, not an exhaustive entity export |
| `search_chunks` | `as_of`, `limit`, `limit_reached`, `chunks[]`. Each chunk has `chunk_id`, `document_id`, zero-based `seq`, `filename`, `text`, `truncated`. The text uses the same 800-character cutoff as the prose |
| `entity_facts` | `entity` (`id`, `name`, `type_key`, `type_label`), effective `at`/`as_of`/`before`, `total_facts`, `matched_facts`, `limit`, `truncated`, `facts[]`, `derived_facts[]` |
| `changes` | Effective `since` (inclusive) and `until` (exclusive), `limit`, `limit_reached`, `changes[]`. Events carry `fact_id`, `at`, `kind`, `subject_id`, `subject_name`, `predicate_label`, `object_name`, `object_value`, `confidence`, `valid_from`, `valid_to`, both validity precisions, `document_id`, `filename`, `quote` |

An asserted `facts[]` entry contains `id`, `direction`, `predicate_key`,
`predicate_label`, `inferred`, `temporal`, `other_id`, `other_name`, `object_value`,
`confidence`, `valid_from`, `valid_to`, `valid_from_precision`, `valid_to_precision`,
`holds_from`, `holds_to`, `recorded_at`, `invalidated_at`, `supersedes`, and
deduplicated `document_ids`. `direction=out` means the requested entity is the
subject; `in` means it is the object. `object_value` retains the JSON value and
unit rather than formatting them into a string. `inferred` describes an
unaccepted predicate name; it does **not** mean the fact is derived.
`qualifiers[]` preserves attributes attached to the relation itself: each has
`qualifier_type_id`, `key`, `label`, raw `value`, `entity_id`, and `entity_name`.

A `derived_facts[]` entry contains `id`, `subject_id`, `subject`, `predicate_id`,
`predicate`, `object_id`, `object`, raw `object_value`, `rule`, `rule_name`,
`rule_id`, `attribute_rule_id`, `confidence`, `valid_from`, `valid_to`, both
validity precisions, `derived_at`, and `invalidated_at`. Rule IDs distinguish
axiom rules from business rules. The derivation ID joins to `…:derived:{id}` in
RDF to follow its proof; this result does not expand the proof tree.

`valid_*` describes stated world time; `holds_*` is the interpreted interval used
for filtering assertions. `recorded_at` (or `derived_at`) and `invalidated_at`
describe the stored record-time lifetime. A later invalidation can be present
even when reading a row held at an earlier `as_of`. `before=T` takes precedence
over `as_of` and reports the effective cutoff `T − 1µs`. A `changes[].at` can be
passed to `before` without losing precision. Events have no separate event UUID;
`fact_id` identifies the affected assertion, and can appear in multiple events.

The structure follows the current text selection: assertion filters and the
default 80-fact limit (maximum 300) apply to `facts[]`; derived conclusions only
use `at`/`as_of`/`before` and are listed separately without that limit. `truncated`
reports omitted matching assertions. Search returns at most six chunks and
changes at most 40 events; `limit_reached` means the cap was reached, not that
another result is known to exist. Narrow the query/window to inspect more.
Historical full-text search retains the recall limitation described above.

## What an agent records waits for a nod

`remember` stores the sentence immediately — searchable at once, attributed to the token's owner. The facts extracted from it do **not** enter the graph. They queue in Review as proposals, each shown beneath the sentence it came from, and a person confirms or rejects them one at a time.

That gate is why the tool can be served at all. An agent reads documents in the base, and a document can say "remember that X" — so an agent may be talked into recording something by the very material it was asked to read. A proposal that waits for a person costs a click; an assertion that does not costs a wrong edge on the graph, indistinguishable from one someone meant.

The Review card names the agent, not only the person: several clients can share one owner's identity, and "which one said this" is most of what a reviewer has to go on. Never claim to a user that a fact was added — say the sentence was recorded and its facts await confirmation.

## Try it from a shell

List the tools:

```bash
curl -s -X POST https://your-utopia/api/v1/kbs/$KB/mcp \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

Search, then read the whole document the best hit came from:

```bash
curl -s -X POST https://your-utopia/api/v1/kbs/$KB/mcp \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call",
       "params":{"name":"search_chunks","arguments":{"query":"Series C target"}}}'

curl -s -X POST https://your-utopia/api/v1/kbs/$KB/mcp \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call",
       "params":{"name":"get_document","arguments":{"document_id":"<id from the hit>"}}}'
```

A document id from another base, or a made-up one, comes back as "No document with that id in this knowledge base" — the tool cannot tell you what it is not allowed to see.

## Point a client at it

Most MCP clients accept a remote server as a URL plus headers. For a client that reads a JSON config:

```json
{
  "mcpServers": {
    "utopia-general": {
      "url": "https://your-utopia/api/v1/kbs/<kb_id>/mcp",
      "headers": { "Authorization": "Bearer utp_pat_…" }
    }
  }
}
```

One entry per knowledge base you want the agent to reach. The agent sees the same base the token's owner sees, and nothing else.

## What is not here yet

- **SQL.** `query_data` (a read-only query against a mounted database) is a chat tool and is not served over MCP. It reaches production databases outside this deployment, and what a borrowed agent should be able to ask them is a question this version does not answer.
- **Streaming.** Responses are single JSON-RPC replies; there is no server-sent event channel.
