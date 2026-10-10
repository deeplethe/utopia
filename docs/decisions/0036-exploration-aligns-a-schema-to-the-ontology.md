# 0036 · Exploration aligns a schema to the ontology

- **Status**: In progress · table alignment proposals, conversion expressions and decision 7 built · open: #556
- **Written**: 2026-09-09 (conventions in the [README](README.md))
- **Related**: [0011](0011-a-mapping-is-not-a-fact.md) moved a mapping out of the ledger and
  is right that it is configuration; this record moves the *concept* out of the entity table.
  [0009](0009-no-type-is-a-type.md), [0010](0010-no-relation-is-no-relation.md) and 0011 are
  one line of work, control flow leaving the ontology; this is the fourth step on it.
  [0003](0003-ontology-growth-loop.md) is the loop exploration joins, with a schema as the
  corpus. [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md),
  [0030](0030-a-rule-may-read-what-a-rule-concluded.md) and
  [0032](0032-a-rule-computes-what-it-concludes.md) are the rules a definition becomes;
  0032's refusal of aggregation over the graph is what makes the database the place a
  definition runs. [0018](0018-the-lakehouse-is-one-protocol-away.md) is why a conversion
  cannot be SQL text. The measuring benches that found this: #501, #503, #520.

> A base mounts an e-commerce order table, fifty-five columns wide, and asks exploration for
> its metrics. Exploration first inserts two classes into the ontology, `Metric` and
> `Dimension`, then reads the schema, then creates twelve entities of those classes —
> `order_amount`, `channel`, `gross_profit` — each with a SQL expression in a side table.
> None of the twelve computes a number the business would recognise (0 of 18 definitions,
> #501). Meanwhile the schema document, ingested as prose, is extracted like any other
> document, and the extractor — shown the two new classes — files every column name as an
> entity: `amt_pay` is now a Metric, `buyer_id` a Dimension, `dw.dim_shop` a Dimension with
> eight facts on it. The graph holds forty concept entities, twelve with a definition,
> twenty-eight that are column names, and nowhere in any of it does it say that the table is
> a table of orders.

## The problem

Exploration builds ontology. It should align to one.

What it builds is the wrong kind of thing. `Metric` and `Dimension` are not classes of
things in the world; they are BI vocabulary for *kinds of column*. The world has orders,
customers and shops. "GMV" is not something that exists; it is an aggregate over an
attribute of orders under a business convention. Making it an entity puts a thing on the
graph that stands for nothing, and 0009, 0010 and 0011 spent three records removing exactly
that species — `concept`, `related_to`, `mapped_to` — from the ontology on the grounds that
they were control flow. The two classes here are the same species, admitted through
`ensure_concept_types` without a proposal, without a person, and (until #231) without even
being noticed.

Three things then have nowhere to live, and the benches measured each:

- **A convention.** "Amounts are in cents; test orders are excluded; an order counts at
  status 2, 3 or 4." Eighteen definitions in the wide corpus share six such sentences. They
  are not an attribute of any Metric, not a field of any mapping row, not a column of
  `knowledge_bases`. With them the model answers 17 of 18 questions; without them, 1 of 18
  (#520). It read every comment and still returned ¥45,992,467 for a GMV of ¥32,931,921,
  because the comments say what a column *is* and nothing says which rows count.
- **An object whose fields sit in several tables.** `(kb_id, concept_id, source)` was meant
  for one number defined two ways; it cannot say that a customer's tier is in the order
  table and their name in the CRM. There is no key, no join, no class.
- **The connection between the graph and the database.** A concept entity has zero facts
  (`facts_on_it = 0` for all twelve); its relation to `dwd_ord_dtl` exists only in the side
  table. Rules (0021–0032) read facts and cannot see a mapping; chat reads mappings and
  cannot see a rule. Two paths, no seam.

## Dead ends

- **Keep the two classes and stop the extractor from seeing them.** Treats the symptom. The
  column-name entities are the visible half; the invisible half is that `order_amount` is
  still a thing that does not exist.
- **A "conventions" text box on the base, read into both prompts.** Proposed during this
  investigation, and it would move the wide corpus from 6% — the model does read prose.
  Rejected because it is a place to put sentences instead of a place to put the thing the
  sentences describe: a filter on an attribute, a unit on a column. Once those have a home
  the box is redundant, and a box that stays is where the next undocumented convention goes
  instead of into the ontology.

  > **Revised 2026-09-09, after measuring it.** The paragraph above rejected the box on
  > principle and gave no number. The number: a one-page markdown of the six conventions,
  > dropped into the "Data schemas" folder (retrieval only, after #553) and reached through
  > `search_chunks`, takes the wide corpus from **2/18 to 14/18** (#520). Zero code, zero
  > mappings. That is two-thirds of the gap to the seeded 17/18, and it says the cheap thing
  > should exist — a person needs somewhere to write conventions *today*, and a document
  > already works. What the four misses show is the reason the principle still stands: two
  > of them are the page contradicting itself (its "refund total" counted fully-refunded
  > orders GMV never held; "valid" and "paid" were both defined and the model divided by the
  > wrong one). Prose lets two definitions be written that do not compose; an expression
  > over attributes cannot be. So the order of work follows: a written definition (#562) now,
  > the alignment and rules for what prose cannot make unambiguous. The dead end is not the
  > box; it is the box as the *only* place.
- **Natural-language mappings.** Not executable, therefore not verifiable: the bench cannot
  run "divide by a hundred". Every question re-translates the sentence, and #520's baseline
  is the record of how that goes.
- **SQL text as the definition** (today's `expr` and `sql` columns). Executable and
  verifiable, and wrong on two counts 0032 already made about rule expressions: it binds to a
  dialect (`FILTER (WHERE …)` is Postgres; there are five engines), and it can name a column
  that does not exist, so the error moves from unfillable to undiscovered.
- **Exploration proposing the definitions themselves** (GMV, refund rate). The schema does
  not contain them — 0 of 18 on the wide corpus, and the model said in its own words that it
  was searching for a definition of "valid order" and found none. What it would propose is
  `sum(amt_pay)`: runs, looks right, off by a hundred and includes the test orders. That is
  precisely what automatic confirmation (#504) must never ship.

## Decisions

### 1. Exploration produces an alignment proposal per table

For each table it reads: the class it is a table *of*, the attributes its columns carry, the
relations its foreign keys are, and for each aligned column an expression that turns the
column into the attribute's value. It finds existing classes and attributes first and
proposes new ones only where none fits — 0003's loop with a schema in place of a corpus.

It goes through `ontology_proposals`, which already has the four sections this needs
(`entity_types`, `attribute_types`, `relation_types`, `map_to`), and a person adopts it. **A
table's alignment is adopted or rejected as one thing.** Adopting `Order` while rejecting
every attribute of it is not a decision anyone means to make, so the page does not offer it.
Adoption writes the ontology rows and the alignment rows together.

### 2. Not every column is an attribute, and exploration must say which class each one belongs to

A wide table is several classes flattened into one. The alignment un-flattens it:

| column | is |
|---|---|
| `amt_pay`, `qty`, `ord_st`, `dt_crt` | attributes of `Order` |
| `buyer_id`, `shop_id` | relations, `Order → Customer` and `Order → Shop` |
| `shop_nm`, `prov`, `buyer_lvl` | attributes of `Shop`, `Address`, `Customer`, flattened in |
| `ver`, `etl_dt`, `rmk`, `price_old` | nothing; left out |

This is the judgement #502 was reaching for (a key is not a quantity) with the rest of it:
a key is a relation, a quantity is an attribute, and a flattened column is somebody else's
attribute. It is harder than what exploration does today and it is the whole value of doing
it.

### 3. A conversion is a tree, the same tree as a rule expression

`amt_pay / 100`, `CASE chnl WHEN 1 THEN 'app' … END`, `date_trunc('month', dt_crt)` are
stored as the expression tree 0032 defined — `{attr} | {const} | {op, l, r}` — extended with
the node kinds alignment needs and rules do not yet: a cast, a case, a date truncation. The
extension is driven by corpora, not designed ahead: the wide corpus needs those three, and
the next corpus says what else.

A tree is engine-neutral and is rendered to each dialect at query time. A tree's leaves are
attribute ids, so it cannot reference a column that does not exist; the failure is at save
time, where a person is looking.

**Input may be text.** 0032 allowed "a box that parses into this same tree", and this is
the case for it: a person and a model both write `amt_pay / 100` more readily than they
compose it from pickers. `sqlparser` is already a dependency (the read-only gate in
`query_engine` uses it); it parses the text into the tree, and what it cannot parse into a
supported node is refused. What 0032 forbade — storing the string and evaluating it at run
time — stays forbidden.

The natural-language column stays, as explanation: `summary` is what a reviewer reads and
what the chat prompt quotes. The tree is the definition, the sentence is the description,
and neither replaces the other.

#### Conversion expressions (#555)

The first conversion nodes are deliberately small: a cast to the existing finite
floating-point number type, a simple `CASE` mapping scalar codes to scalar values,
and truncation to a UTC calendar year, month or day. `CASE chnl WHEN 1 THEN 'app'
ELSE 'other' END` translates a representation and belongs on the alignment. A
named business grouping still belongs in a rule; using the same expression there
does not make exploration its author. Case arms are literal numbers or text, not
predicates or subqueries. This keeps each conversion's input readings and temporal
premises explicit. A missing input, an unmatched case without `ELSE`, an invalid
date or a failed numeric conversion produces no value, never a substitute zero.

Numeric constants retain `const`, including the previously accepted numeric
strings. Text literals use `text`, so admitting labels does not turn a misspelled
numeric constant into a valid number. The additional shapes are:

```json
{"cast":"number","expr":{"attr":"<attribute UUID>"}}
{"case":{"attr":"<attribute UUID>"},"when":[{"is":1,"then":"app"},{"is":2,"then":"web"}],"else":"other"}
{"date_trunc":"month","expr":{"attr":"<attribute UUID>"}}
```

Case results have one scalar type, and at most 32 arms. The existing four-edge
`MAX_EXPR_DEPTH` bound applies to every expression. Dates accept an ISO calendar
date or an RFC 3339 timestamp with an offset; timestamp offsets are converted to
UTC before truncation. The result is an ISO calendar date, such as `2024-02-01`.
Naive timestamps, arbitrary cast types, decimal precision/scale, searched cases,
aggregates and other SQL functions are outside this cut.

Text is an input format only. The rule API can accept an expression as
`{"expression":"CAST(amount AS DOUBLE PRECISION) / 100"}` wherever it accepts
an expression tree. It resolves attribute **keys** in the current base and stores
the resulting UUID tree; unresolved names and unsupported syntax fail before the
rule is written. Ordinary scalar threshold strings keep their existing meaning.
This gives the conversion tree an executable consumer before table alignments
exist. Reading, validating and evaluating JSON share the same node definitions;
rule results and their existing premise/interval machinery can carry text and
calendar dates as well as numbers. A bare attribute expression retains its
existing numeric interpretation. The read-only rule display follows the tree;
the experimental picker is not enabled by this change.

Table/column binding and adoption are described below. Rendering the tree into each
source dialect and retiring the old mapping classes remain #556. This cut does
not reinterpret existing SQL mappings or decide the unit policy proposed in 0049.

#### Table alignment proposals (#554)

Exploration now writes one `ontology_proposals` item in the `table_alignments`
section per mounted source and table. Its payload groups new class, attribute and
relation declarations with every column's owner and property, conversion tree or
relation target, and the reason for each omitted column. Existing vocabulary is
given to the model first. Exploration itself creates no ontology or entity rows
and proposes no aggregate definitions. The manual SQL definition path remains
available while #556 is pending.

Column names in expression text resolve against the table's aligned attribute
columns, using the same parser and tree as rule input. The stored proposal has
provisional attribute identities; adoption resolves the ontology keys in its
transaction and substitutes the actual attribute IDs. Each conversion also stores
`input_columns` (attribute ID → raw column), so a shop's name and a customer's name
can reuse one ontology attribute without sharing a raw reading. A single
conversion cannot read two columns bound to that same attribute: the current tree
cannot distinguish the two leaves. Inputs are raw readings, not the outputs of
other column conversions. Flattened owners without their own key column, such as
an address's province, retain their class; deriving joins or row identities is
outside this alignment writer.

`GET /api/v1/kbs/{id}/table-alignments` returns complete proposals and the latest
exploration report. An editor posts `key`, `version` and `status` (`adopted` or
`rejected`) to `/table-alignments/decision`. Adoption re-reads the source schema,
then locks the proposal and checks its version, mount and current ontology.
Classes, properties, table and column bindings, alignment jobs and the decision
commit together. Missing columns, incompatible reused signatures or any write
failure leave the proposal open. Refreshing an open proposal changes its version;
an old review cannot accept the new payload. Re-exploration preserves adopted and
rejected decisions. Unmounting removes active bindings and hides that source's
proposals. The status-only ontology endpoint cannot adopt a table.

The Data mapping page reviews these groups under Table alignments, showing the
column owners, conversions, exclusions and proposed vocabulary before the two
whole-table actions. Viewers can read them. Editing a conversion and re-adopting an
already decided alignment are not part of this cut; an open proposal can be
regenerated by exploration. Existing SQL definitions retain their own review flow
and remain what Ask reads.

The database tests use a scripted model and the real Postgres schema reader to
check cold start, conversion binding, atomic rollback, concurrent decisions,
re-exploration and schema drift. `mappings.mjs --alignments --corpus wide` scores
table classes, column owners/properties, foreign-key roles and exclusions against
explicit key alternatives in the truth file. It is a structural score; different
valid vocabulary needs human review, and it does not measure a model's conversion
accuracy or claim results on a live provider.

### 4. A definition is a rule over aligned attributes, and a person writes it

GMV is `sum(Order.paid_amount) where Order.is_valid`. It is not an entity and not a row in
a mapping table; it is a rule of the kind 0021 built and 0032 made computable, over
attributes the alignment made real. Exploration prepares the attributes; it does not propose
the rule (dead ends, above).

A convention that many definitions share is **one rule that the others read** (0030):
`Order.is_valid ← status ∈ {paid, shipped, done} ∧ ¬is_test`, written once, inherited by
GMV, net sales, order count, refund rate. This is where "test orders don't count" lives — a
condition on an attribute, with a name, that a person wrote and can change in one place.

The unit conversion lives on the alignment (`amt_pay / 100`), and the attribute carries the
unit (`relation_types.unit = 'CNY'`). 0032 noted that nothing checks units when an
expression is written; this record gives units a place to come from, so that check has
something to read.

### 5. `Metric` and `Dimension` retire; `concept_mappings` becomes derived

`ensure_concept_types` goes. No exploration inserts a class without a proposal.

`concept_mappings` is not dropped: its status, revisions, audit stream and page (0011 §2–3)
are the review flow this record still needs. What changes is where a row comes from. Today a
row is the definition; after this, a row is **rendered** from an alignment and a rule — the
rule, the table it aligns to, the column expressions, the source's dialect — into the `sql`
it holds now. The bench (#501, #520) reads the same table it reads today and scores the same
way. A row that a person edited by hand and a row that was rendered are distinguishable,
because a rendered one names the rule it came from.

### 6. Cold start: exploration proposes the class it cannot find

A new base has no `Order`. Exploration proposes it, through the same proposals and the same
adoption, which is the difference from today: `ensure_concept_types` writes; a proposal
waits. A base on an ontology pack (0008) will more often find `Order` already there and
align to it, which is the case this record is written for.

### 7. The schema document is a search corpus, not a source of facts

Ingesting a schema as markdown gives chat something to retrieve (`search_chunks` finds the
table). Extracting it as if it were prose is what produced twenty-eight column-name
entities. The document keeps its place in retrieval and leaves the extraction path. This is
the one decision here that is a bug fix and can land on its own.

## Open questions

- **Where a rule over aligned attributes runs.** 0032 refused aggregation on the graph
  because the graph is open-world and a sum asserts completeness. A database table is
  closed-world: the table *is* all the rows, so `sum(Order.paid_amount)` is an honest
  statement there. The alignment therefore gives a rule a place it can legitimately be
  summed — but that place is another executor, translating the tree and the rule to a
  dialect and running it at the source. Whether to build that, or to stop at rendering
  `concept_mappings` rows for chat to use, is the largest decision this record does not
  make. Today the two paths (documents → facts → rules; database → schema document → model
  writes SQL) are separate, and this record only says where the seam would be.
- **Matching a column to an existing attribute.** By name? By sampled values (#502)? By
  asking the model to say, with the attribute list in the prompt? The wide corpus, where
  `amt_pay` must land on `Order.paid_amount` and `buyer_lvl` on `Customer.tier`, is the test.
- **What another corpus needs beyond the conversion subset above.** A code-to-label
  case is a conversion; a named business grouping is a rule that may reuse it.
- **The page.** How a person reviews a table's alignment, edits one column's expression,
  and sees which rules a change reaches. Capability first, interface after, as usual.
- **`derived`** on `concept_mappings` (0011 §1) was for "conversion rate = orders / visits".
  Under this record that is a rule reading two rules, and the flag has no meaning. It is
  kept until the rendering exists and dropped with the migration that consolidates it.

## What this does to 0011

0011's argument stands: a mapping is not an assertion about the world; it is configuration;
it lives outside the ledger and may be edited in place. This record accepts all of that and
changes one thing 0011 did not decide but its implementation assumed — that the *concept* a
mapping hangs from is an entity of a class called Metric. The concept is an attribute of a
real class, or a rule over such attributes, and the mapping is how a column becomes that
attribute's value.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> written · decision 7 implemented (#553 → #561: the schema document is indexed and never extracted, `sources.config.extract`, `graph_status = skipped`; the column-name entities went from 93 to 12 on the wide bench base) · a definition can be written by hand (#562 → #563), the door the seeded upper bound simulated · the conventions-as-prose dead end was measured at 14/18 and revised in place (see Dead ends) · cuts remaining: #554 alignment, #555 the conversion tree, #556 retiring the two classes · overturns where a mapping hangs, and keeps what [0011](0011-a-mapping-is-not-a-fact.md) said about what a mapping is · the two builtin classes `metric` / `dimension` are to retire (their revision notes are on 0009 and 0011) · the migration that carries the exploration ledger (#503) is unaffected
