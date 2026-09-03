# 0017 · A contradiction points at an error upstream

- **Status**: implemented · B2a (#238): engine and queue — `derive::contradictions`, migration 0020, the Review card with clues and repairs · B2b: contested edges in the alert colour and ghost edges for blocked derivations on the graph, the disputed chip on panel rows, the "did not land" section of the Derived tab with its proof chain · B2 of 0016, wider than the one line written there: contradictions become visible everywhere, not only as a new kind in the queue
- **Written**: 2026-09-03 (conventions in [README](README.md))
- **Related**: the two unbuilt rows of the "derived vs asserted" table in [0002](0002-reasoning-engine.md) §2; [0016](0016-close-the-open-seams-before-cutting-new-ones.md) B2; the proof chain (B1, #227) supplies the "premises expand to the sentence" half of the card below

> This record changes one phrase in 0002. There, a contradiction "goes to Review for a ruling".
> In discussion (2026-09-03) it became clear what a person actually does when two edges
> disagree: follow the signal to **where something went wrong upstream** — a fact went
> stale, extraction misread, resolution merged the wrong pair, the ontology is too strict.
> So this queue is built as an audit, and every button is a repair.

## The problem

R1 derivation only yields to "this exact triple is already asserted". It does not consult
axioms: `part_of` transitivity derives `A part_of C` while the ledger asserts `C part_of A`
(asymmetric); `ceo_of` is functional and `Acme ceo_of Zhang San` is asserted, yet the
engine still lands `Acme ceo_of Li Si`. R0 scans `facts` only, derivations live in their
own table, so this class of contradiction is **invisible on both sides**: absent from
Review, absent from the graph.

It is not the only invisible class. Temporal conflicts and axiom violations are known to
the Review page alone; on the graph and in the entity panel a contested edge looks exactly
like any other.

## Criteria

1. **A contradiction points at an error upstream.** The error is in one of four places:
   stale knowledge (most common), a misread extraction, a wrong merge, an over-strict
   ontology. The interface's job is to lay out the clues and offer the repairs.
2. **Write less to the graph rather than write wrong** (0002). A derivation that hits a
   contradiction does not land; when two derivations collide, neither lands.
3. **Anything reported item by item needs an upper bound.** Only "one assertion against one
   concrete edge" — a volume that can be predicted — goes into the queue individually, and
   with a per-predicate cap; whatever is produced in batches is aggregated by its cause.
4. **A disputed fact is visible where it sits.** The assertion stays live, but someone
   passing it on the graph or in the panel should see that it is questioned, and reach the
   place to fix it in one step.

## Decisions

### 1. Detection: two tiers, pure logic in `utopia-reason`

`derive::contradictions(derived, edges, axioms)` checks every derivation against the axioms
of its predicate, once against assertions and once against other derivations:

| Axiom | What counts as a contradiction |
|---|---|
| functional | same subject and predicate, different object, **validity intervals overlap** (Mira leaving and Devin taking over is a succession; the half-open interval semantics of `validity` are reused) |
| inverse_functional | the dual |
| asymmetric | the reverse edge exists with overlapping validity |
| irreflexive | the derivation is a self-loop |

**Derived vs asserted**: per item. The derivation does not land; one `axiom_violations`
row of kind `derived_contradiction`. Capped per predicate (same shape as R1's
`MAX_DERIVED_PER_PREDICATE`; 50 suggested); the overflow stays out of the queue and is
counted in the report.

**Derived vs derived**: aggregated. The cause is a rule set that contradicts itself —
`ceo_of ⊑ works_at` together with `works_at` functional necessarily produces contradictions
in batches. Queuing every pair would flood Review, and a person facing a hundred identical
"two derivations disagree" cards cannot decide anything from them. So one row per **pair of
rules** goes into `ontology_defects`, kind `rules_disagree`, recording both rules, the
count, and two or three examples. It is the dynamic form of the existing static defects
`transitive_and_functional` and `symmetric_and_asymmetric`: invisible in the declarations,
surfacing once data arrives. Neither derivation lands.

`run()` (R0) and `materialize` (R1) share the one function: R0 already holds the edges and
axioms and adds a `derive` + `contradictions` pass to report; R1 uses the same result to
decide what stays unlanded. **Both must compute it** — otherwise R0's "clear open rows not
recomputed this round" would sweep away what R1 wrote.

### 2. The Review card: lay out the clues, lay out the repairs

**The derived-vs-asserted card**, three parts:

1. **The contradiction itself**: the derived edge (rule, premise chain expandable to the
   sentence — from B1) beside the asserted edge (evidence quote, interval, confidence).
   Most errors are visible at a glance.
2. **A diagnostic hint**, one line when a clue can be computed, none when it cannot:
   - the assertion has no end date and the derivation starts after it → "looks stale: did
     Zhang San's tenure end in 2024-07?"
   - an entity on either side has same-name neighbours (the existing `same_name` machinery)
     → "looks like a wrong merge: are these two Acmes one company?"
   - the assertion's confidence is below 0.75 → "the extraction was unsure to begin with"
   - none of the above → "read both sentences"
3. **Actions that are repairs**, all on existing endpoints; once a repair is made the
   violation clears on the next recomputation, with no separate resolve step:

| Action | What it repairs | Where it lands |
|---|---|---|
| Close the assertion at a date | stale knowledge | `POST /facts/{id}/close` (same as the temporal-conflict queue, with a date input) |
| Retract the assertion | misread extraction / wrong merge | `reject_fact`, append-only |
| Go to possible duplicates | wrong merge | the Duplicates queue in Review |
| The ontology is too strict | wrong definition | the relation on the Ontology page |
| Both hold; let the derivation through | the world is like that | `resolution = accepted`; the next `materialize` lets that pair land |

**The derived-vs-derived card** (in the ontology-defects queue): "these two declarations
together produced N contradicting pairs; examples below — usually a sub-property attached
in the wrong place, or a functional declaration that is too strict". Two actions: go to the
Ontology page and change a declaration; or "accept" (contradicting derivations from this
pair of rules never land and are not reported again). Both sides are derived and the
assertions beneath may each be right, so there is no "the data is wrong" here; "both hold"
would mean giving up the meaning of functional, which belongs on the Ontology page, so
there is no such button either.

### 3. Disputed facts are visible where they sit

"Disputed" becomes one status with three sources: open temporal conflicts
(`fact_conflicts`), open axiom violations (`axiom_violations`, including the new kind), and
blocked derivations (the `detail` of `derived_contradiction` rows). B2 makes it a first-class
state of the browse pages and wires up the first two sources along the way — they are
equally invisible today.

**Rows in the entity panel**: `EntityFact` already carries `stale` and `corrected`; a third
flag `contested: Option<Contest { kind, ref_id }>` is computed with one EXISTS. The chip
reads "disputed"; hover gives one sentence ("derived works_at Li Si contradicts this");
click goes to the matching Review item. The row is not dimmed: the assertion is still live.

**The Derived tab of the panel**: a new section, "derivations that did not land", read from
`axiom_violations.detail`, each row naming what blocked it and expanding to its premise
chain. Here a person sees "the engine could have drawn this edge, and what stopped it".

**Edges on the graph**:

- A contested assertion switches to the **alert color**, edge and all, with no reliance on
  a node ring. The request was explicit: a ring sits on the node while the edge stays gray,
  and peripheral vision cannot tell them apart.
- A blocked derivation is drawn as a **ghost edge**: the same hue mixed toward `EDGE_DIM`
  (under premultiplied blending alpha cannot darken an edge; only the RGB can be mixed — see
  the note at `EDGE_DIM`), thinner. It follows the Derived toggle; clicking it opens the
  entity panel at that row. sigma's default edge program does not draw dashes, and no custom
  program is introduced for this.
- The hover label chip gets a "⚠" prefix, and the tooltip states the dispute.

**Color**: one new token, `--u-contest`, set to **#ff6a3d** (hot coral orange). It has to
stand apart from three colors already in use: derived edges are gold
(`rgb(231,197,124)`), warning chips are amber (`--u-warn` #f2b66d, too close to gold to
double as an edge color), and danger is pink (`--u-danger` #ff9daf, reserved for
destructive actions). Coral orange is bright enough on the dark ground, its hue is far from
all three, and it is no common type color (the type palette leans blue, green and violet).
Edge color `rgba(255,106,61,0.55)`; ghost edge `lerp(#ff6a3d, EDGE_DIM, 0.55)`. This is
the only new color; chips on the Ontology and Review pages use the same token.

### 4. Data

Migration `0020` (0019 was taken by #199 / #233):

- `axiom_violations.kind` CHECK gains `derived_contradiction`
- `axiom_violations` gains `detail JSONB NOT NULL DEFAULT '{}'`: the derived triple (three
  ids and three names), the rule kind, the premise ids — without it the interface cannot
  say what was derived; `path` holds the premises for the proof chain but cannot draw the
  ghost edge
- `axiom_violations.resolution` CHECK gains `fact_closed` (closing an assertion is a repair
  that leaves its own trace, kept apart from `fact_retracted`)
- `ontology_defects.kind` CHECK gains `rules_disagree`, plus `detail JSONB` (both rules, the
  count, the examples)

`left_fact` / `right_fact` stay non-null and keep pointing at `facts`: for derived vs
asserted, `left` is the contradicted assertion and `right` the derivation's last premise;
`rules_disagree` does not live in this table.

### 5. Interfaces

- `GET /kbs/{id}/graph`: edges gain `contested: bool`; a new class of edge with
  `blocked: true` (the ghosts)
- `GET /kbs/{id}/entities/{id}`: `facts[].contested`; `blocked: [...]` next to `derived`
- Review `violations` queue: rows gain `detail` and `hint` (the diagnostic as a code; wording
  belongs to the frontend)
- `POST /kbs/{id}/review/violations/{id}`: `resolution` gains `fact_closed` (with
  `close_at`); the server calls `close_fact` and then marks the row resolved

### 6. Tests

- `utopia-reason` unit tests: one per axiom, non-overlapping intervals are no contradiction,
  derived-vs-derived aggregates by rule pair, the cap is counted
- Store integration: a functional assertion stands and a derivation collides → the
  derivation stays out and one violation carries `detail`; retract the assertion and rerun →
  the derivation lands and the violation clears; `accepted` → both coexist; `fact_closed`
  runs the close path; R0 and R1 agree
- Browser: an edge turns coral, ghost edges follow the toggle, the panel chip jumps to the
  Review item, the Review card shows three parts and five actions

## Two cuts

**B2a · engine and queue**: §1, §2, §4, and the Review parts of §5, with the integration
tests. Two days.
**B2b · visibility**: §3 plus the graph and panel interfaces, wiring existing temporal
conflicts and axiom violations along the way. A day and a half.

B1 (#227) merges first after a rebase; B2a branches from it.

## Open questions

- **The per-predicate cap of 50 is a guess**, awaiting a bench number like R1's twenty
  thousand.
- **Granularity of `accepted`**: per pair (one derivation against one assertion). An
  assertion hit by several derivations needs several clicks; aggregating the exemption per
  assertion would generalize "this axiom does not apply to this assertion" too far. Per
  pair first, measure later.
- **Could ghost edges be too many?** Their number is bounded by the (capped) violation
  count, so it should stay manageable; failing that, draw them only when a related node is
  selected.
- **Does the disputed status need its own SSE event?** The `review` event is already sent;
  graph and panel can refetch on it. No new event.
