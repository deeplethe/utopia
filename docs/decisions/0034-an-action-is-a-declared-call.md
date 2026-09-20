# 0034 · An action is a declared call

- **Status**: cut 1 · this record. No code yet
- **Written**: 2026-09-08 (conventions in the [README](README.md))
- **Related**: [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md) built the rule whose conclusion will one day fire an action; this record builds the thing it will fire and stops there. [0032](0032-a-rule-computes-what-it-concludes.md) drew the line this record keeps: everything a person authors is structured, so the page shows what runs. [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) is why an agent will not call one without a nod. [0020](0020-an-auditor-reads-it-without-us.md) is why every run is a row. The managed fetch path (#330, `http_fetch`) is the road a call travels. The grant layer follows what `data_source_grants` did for warehouses.

> A base that knows a well is gas-bearing, or that a contract lapsed last month, has no way to say so outside itself. The person who wants a ticket opened, a message posted or a record updated in another system reads the panel and does it by hand. When a rule reaches the same conclusion on its own (0021), there is nothing it could call. The base has readers and a ledger. It has no hands.

## What the ground already gives, and what it withholds

Four parts are reusable as they stand:

- **A managed way to reach a URL.** `http_fetch` resolves a host once and pins the addresses, follows redirects by hand and re-checks each hop, refuses an https → http downgrade, caps the body and the whole trip, and carries a `Reach` that says whether an intranet host is a legitimate target.
- **Sealed secrets at rest.** `secrets::seal` / `open` already hold the model keys, the warehouse connection strings and the source credentials.
- **A grant layer with a precedent.** A warehouse is registered once by the deployment admin and granted to the workspaces that may use it; a base cannot reach one it was not granted.
- **A ledger.** `audit_events` for who did what, the job queue for anything that must retry.

What it withholds: no table holds "a call this deployment can make", no place holds a typed parameter, and nothing records what the base sent to the outside world.

## Decisions

**1. An action is data.**

An action is a name, an HTTP method, a URL, a set of headers, an auth block, a JSON body template and a list of parameters. The only computation is substitution: `{name}` in the path, in the query, in a header value or in a body string. A body string that is exactly one placeholder takes the parameter's typed value (a number stays a number); a placeholder inside a longer string becomes text. No expressions, no conditionals, no script.

The reason is 0032's. The registry page renders the action from the same structure the runner reads, so the two cannot drift. And a placeholder that names no parameter is refused when the action is saved; a script would have sent an empty string at run time and nobody would have noticed until the other system complained.

**2. Registered once for the deployment, granted to a base.**

An action belongs to the deployment and is written by a deployment admin. A base may call it only after being granted it; the grant is a row, given and taken back by the deployment admin. Revoking a grant stops one base. `enabled = false` stops every base.

Why the deployment level: an endpoint with a credential is the deployment's relationship with another system. The same ticketing system serves every base, and the person who holds the credential should be the one who registers it, once. A first draft of this record put the registry inside the base beside the rules; it was withdrawn because it would have made every base admin a holder of external credentials and every shared endpoint a copy.

Why one layer where the warehouses have two: a warehouse is *mounted* because mounting does work (its schema is ingested into the base). An action has nothing to configure on the base's side, so the grant is the whole of it.

**3. A parameter is a scalar: a number, a string or a boolean.**

A number may carry a lower and an upper bound. A string may carry the set of values it accepts. Every parameter says whether it is required. Nothing nests: a parameter is never an object or an array. Shape lives in the body template and values live in parameters; a nested parameter would need a schema language of its own, and the template already says what the endpoint wants.

Checks run before anything is sent: an argument the action did not declare is refused (it is almost always a typo), a required one that is missing is refused, a number outside its bounds and a string outside its set are refused, and a boolean has to be one.

**4. Every run is a row.**

`action_runs` records which action (by id, and by name so the row still reads after the action is deleted), which base, who, the arguments, what was sent (method, URL, header names, body; never the auth block), the status, the duration, the first 4 KB of the response and the error if there was one. `trigger` is `person` in this cut; `rule` and `agent` arrive with their own records and their own columns.

Two log pages read the same table: the deployment's, across every base, and each base's own. The reason is 0020's: what the base did to the world is the part of the ledger an auditor asks about first.

**5. The reach is the operator's.**

The URL was typed by a deployment admin, so an intranet host is a legitimate target (`Reach::Operator`), with everything else that path enforces: pinned addresses, re-checked redirects, no downgrade, a body cap of 1 MB and a deadline of 15 seconds. Two rules are stricter here than for a source. A placeholder may not appear in the host: a parameter in the host is a request to reach anywhere, and every rendered value is percent-encoded into the path or the query. A rendered header value may not contain a line break.

**6. Three roles.**

The deployment admin writes, grants and may test-run from the registry (a run with no base, logged as such). A base's Editor runs a granted action, because a run has effects outside the base. A base's Viewer sees the granted actions without the auth block, and the base's log.

## The seam for rules

Not in this record, but the shape it has to fit is worth writing down now so nothing here forecloses it. A rule gains an optional action and a binding per parameter: a literal, the subject's name, a premise's value or the concluded value, checked against the parameter's kind and bounds when the rule is saved. It fires when a derived row **first appears**, not on every materialization pass: 0030 recomputes in full, and the identity index already decides which rows are new. It runs through the job queue with retries and a dedupe key of `(rule_id, derived_fact_id)`, so a pass that recomputes the same conclusion sends nothing. A conclusion that retires may fire a second action. An agent calling an action is a write over MCP and waits for a nod (0015).

## Schema

```sql
actions        id, name (unique in the deployment), description, kind ('http'),
               method (GET | POST | PUT | PATCH | DELETE), url,
               headers JSONB (plain, shown back), auth TEXT (sealed JSON:
               none | bearer {token} | basic {username, password} | header {name, value}),
               body JSONB (template, nullable), enabled, created_by, created_at, updated_at
action_params  id, action_id (cascade), seq, name (^[a-z][a-z0-9_]{0,39}$, unique per action),
               kind (number | string | boolean), required, description,
               min_value, max_value (number only, min <= max), options TEXT[] (string only)
action_grants  action_id (cascade), kb_id (cascade), granted_at, granted_by
action_runs    id, action_id (set null on delete), action_name, kb_id (set null; null = a registry test),
               trigger ('person'), actor_id, args JSONB, request JSONB {method, url, header_names, body},
               status, ok, error, response_excerpt, duration_ms, started_at, finished_at
```

Parameters are a table rather than a JSON column so a rule binding can later point at a parameter by id and survive a rename, and so the checks above are constraints the database keeps rather than code the store remembers to run. The auth block is its own column rather than a header among headers so that only one place accepts a secret; the plain headers are shown back in full and edited in place.

## API

```
GET    /admin/actions                               registry, with grant counts and the last run
POST   /admin/actions
GET    /admin/actions/{id}
PATCH  /admin/actions/{id}                          auth and params replace as a whole when given
DELETE /admin/actions/{id}
PUT    /admin/actions/{id}/grants/{kb_id}
DELETE /admin/actions/{id}/grants/{kb_id}
POST   /admin/actions/{id}/preview   {args}         the rendered request, not sent
POST   /admin/actions/{id}/run       {args}         a registry test, logged with no base
GET    /admin/action-runs            ?action &kb &ok &page &per

GET    /kbs/{id}/actions                            the actions granted to this base   (Viewer)
POST   /kbs/{id}/actions/{aid}/preview  {args}                                          (Viewer)
POST   /kbs/{id}/actions/{aid}/run      {args}      runs, then one row                  (Editor)
GET    /kbs/{id}/action-runs            ?action &ok &page &per                          (Viewer)
```

A run is synchronous in this cut: a person presses Run and waits for the row. When rules fire, the same `run_action` is called from a job.

## Phasing

1. **Capability.** Schema, `utopia-store::actions`, the runner in `utopia-server` (render, send, record), the routes, this record. Tests: the store's (create, grant, a viewer never sees the auth block, a run lands as a row) and the runner's against wiremock (rendering by kind, a placeholder in the host refused, an unknown argument refused, a bound enforced, a redirect into the intranet from a public host refused, the body cap, the deadline).
2. **Administration.** Two entries in the administration rail: **Actions** (the registry as a table, a dialog to write one, a dialog to grant it, a test run) and **Action log** (the deployment's runs, filters outside the panel, a row opens to show the request and the response). A base gains an **Actions** tab: what it was granted, a Run dialog built from the parameters, and the base's own log.
3. **Rules fire actions.** Its own record.

## Dead ends

- **A function runtime** (a JavaScript or Rhai sandbox). The most capable option, and the one that hands the credential to a script and makes the page a description of the code rather than the code. 0002 and 0032 already said why.
- **A fixed body convention** (POST the arguments as one JSON object). It saves the template and fits almost no endpoint: a ticket wants `{fields: {summary}}`, a chat message wants `{text}`.
- **A registry inside the base.** The first draft; withdrawn for the reasons under decision 2.
- **Nested parameters through JSON Schema.** It would let a parameter be an object; it would also make the run form a schema-driven form builder and the rule binding a path language. The template holds the shape instead.
- **The response written back as a fact.** A call that returns a value the base should keep is a source, or a rule (0032); mixing it into an action would make a run both a side effect and an assertion, and the ledger could not say which.

## Open questions

- **Retention.** `action_runs` grows with every call and keeps 4 KB of each response. Nothing trims it yet; a per-deployment retention window is the likely shape.
- **Retries and idempotency** once a rule fires. A person who presses Run twice meant it; a rule that fires twice did not. The dedupe key above is the first answer, and a run that fails with a 5xx probably wants a bounded retry, but neither is decided until there is a caller.
- **A secret in an argument.** Arguments are logged in full. A parameter is therefore never the place for a credential, and the run form should say so; whether a parameter can be marked "not logged" is a question for whoever first needs it.
- **A second kind.** Email, or a tool over MCP, would share the parameters, the grants and the log and differ only in the runner. The `kind` column is the door; each kind is its own record.
- **Who asks for a grant.** A base admin who wants an action has to find the deployment admin. A request queue is the shape data sources never grew either.
- **Naming.** `audit_events.action` names what a person did; an action here is a declared call. The two words meet only in the ledger, where a row reads `action.run`.

## Proposed revision · 2026-09-21 (not accepted or implemented)

An isolated protocol experiment and the exact decisions requested are recorded in
[the experiment report](../../scripts/prototypes/actions/README.md).
This proposal does not change the accepted decisions or implementation status above.
It must be reviewed before enabling the corresponding production write/sender path.
