# A base reads mounted data and acts through declared calls

Records: [0018] (query engines), [0011] (mappings), [0036] (alignment of schemas), [0034] (actions,
proposed), [0016] D, [0014] (`query_data` over MCP). The rules a definition becomes are in
[rules](rules.md).

## What it does today

**Data sources are registered at the deployment and granted to workspaces** (`data_source_grants`,
#142); a base mounts a granted source, which ingests its schema as a document for retrieval
[0011, 0018, 0036 d7]. The connection string is the only input: the scheme picks the engine
(`postgres`, the MySQL wire family since #303, `trino`, `databricks`, `snowflake`); credentials sit
in the userinfo, the path is catalog / database / schema, switches in the query string [0018].

**Four gates on a query.** Parse with the engine's dialect and admit one SELECT only; wrap a LIMIT;
a read-only session with a timeout (the HTTP family has no session, so a timeout alone); JSON Lines
out with numbers and booleans restored from the vendor's column types [0011, 0018]. Loopback and
`NO_PROXY` hosts go direct; everything else follows the proxy variables [0018]. Trino has run
against a real cluster; Databricks and Snowflake are covered by protocol replays and await one
(#241, #242); MaxCompute waits [0018].

**The semantic layer.** `concept_mappings` (concept, source, table, expr, sql, unit, summary,
status, revisions) is configuration outside the ledger with its own page, proposed by exploration
and confirmed by a person; rerunning exploration does not erase a rejection; a Viewer reads, an
Editor revises; chat reads confirmed mappings into its prompt, capped at 30 [0011]. A definition
can be written by hand (#563), and a one-page conventions document in the schema folder moved a
bench from 2 to 14 of 18 with no code [0036]. The concept exploration used to create (`Metric` and
`Dimension` entities with a SQL expression in a side table) is the wrong kind of thing and is to
retire [0036, 0009].

**Actions** are recorded and not built [0034].

## Why

- **Engines follow protocols, not product names**: Iceberg, Delta and Hive are Trino catalogs; the
  binary carries no native driver; reading Iceberg directly is another product [0018].
- **The scheme picks the engine**, so three engines add no dropdown [0018].
- **A mapping is not a fact**: it answers how a number is computed, not what exists; confirming it
  was an UPDATE on an append-only table; its fields hid in JSON [0011].
- **A concept is an attribute of a real class or a rule over attributes**, not an entity:
  exploration filed 28 column names as entities and proposed 0 of 18 usable definitions; a
  convention like "test orders do not count" had nowhere to live (17 of 18 with it, 1 of 18 without)
  [0036].
- **SQL text as a definition binds to a dialect and can name a missing column**; a tree over
  attribute ids fails at save time and renders to each dialect [0036, 0032].
- **An action is data, substituted and never scripted**, registered once for the deployment and
  granted to a base, every run a row, the reach the operator's [0034].
- **A response is never written back as a fact**: that would make a run both a side effect and an
  assertion [0034].

## Proposed and not built

- **Alignment of a schema** [0036]: exploration proposes per table the class it is a table of, its
  attributes, its foreign keys as relations, and a conversion tree per column (0032's tree plus
  cast, case and date truncation; text parsed by `sqlparser`, never stored as SQL), adopted through
  `ontology_proposals` as one thing; a definition is a rule over aligned attributes written by a
  person, with a shared convention as one rule others read; `concept_mappings` becomes rendered
  from an alignment and a rule; `Metric` / `Dimension` retire (#554, #555, #556).
- **Actions** [0034]: `actions`, `action_params` (scalar, bounded, with value sets),
  `action_grants`, `action_runs`; preview and run endpoints for the deployment and for a base; three
  roles; rules firing actions on a derived row's first appearance with a dedupe key; an agent's call
  waits for a nod.
- Mappings leave the Review page for the data-source side [#725]; `concept_mappings.evidence`
  [0016 D1]; the MaxCompute connector; Snowflake key-pair and Trino Kerberos when someone needs them
  [0018].

## Open questions

- Where a rule over aligned attributes runs: at the source, where a sum is honest, or only as
  rendered SQL for chat [0036].
- Matching a column to an existing attribute (by name, sampled values, or the model) [0036].
- How much schema to fetch from a lakehouse catalog (cap 200 tables today); Snowflake's `fixed`
  scale losing trailing zeros [0018].
- Retention of `action_runs`, retries when a rule fires, secrets in arguments, a second action kind
  [0034].
