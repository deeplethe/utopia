# 0012 · The ontology is a contract

- **Status**: implemented · five controlled runs on `ai-timeline-ends` × schema.org + W3C Org took the violation rate from 57% to 4% and true reversals from 39 to 0 · the write-time judgment (`ontology::judge_direction`) now also covers adoption and merge (#190 / #196; merge reports `signature` rows into `axiom_violations`, R0 re-checks the same class) · filtering reified-shell relations out of pack import is still not done · since this record the ontology can also declare `inverseOf` / `subPropertyOf` (#177 / #179) and the pack list grew to five; the runs cover only the first two
- **Written**: 2026-08-31 (inferred: the source is undated and sits between 0011 and 0013, both dated 2026-08-31) · condensed into English 2026-09-03
- **Related**: [0008](0008-ontology-packs-as-cold-start.md) built the packs but never asked whether a large ontology holds up on real text; criterion 2 of [0001](0001-ontology-import-and-governance.md) ("the ontology guides, it does not enforce") is overturned by half here; [0010](0010-no-relation-is-no-relation.md) supplies the empty-predicate behavior; [0013](0013-a-source-should-hand-over-its-history.md) is the other end of the same line — how things come in, versus the rules they land by

## The problem

The packs earn their keep. Six Wikipedia articles with schema.org + W3C Org (1655 relations,
973 classes) and zero seeds: all 215 predicated facts and all 41 distinct relations came from
the pack, and the empty-predicate share fell to 25.6% (55.5% `related_to` in the ten-seed days).

But not one declared constraint was enforced. schema.org declares
`employee (organization → person)`; the graph said `Elon Musk --employee--> Microsoft`, and
102 of 130 checkable facts were written backwards — the reason for choosing schema.org, 1488
properties with declared domain and range, went unused. An empty predicate is honest silence;
a reversed edge is a confident error, exactly the input the engine of
[0002](0002-reasoning-engine.md) amplifies.

One root cause, two symptoms. After #128 removed the seeds, `seed_classes` was empty and
retrieval favored leaf classes that appear literally in the text (in a chunk about Sutskever,
`researcher` ranked 4th of 976 classes, `person` 359th, `organization` 795th). Entities got
typed `researcher` (a subclass of `Audience` in schema.org), and `sig_of`, which only knows
classes that were laid out, degraded the signature of `employee` to `(* → *)`.

## Decisions

1. **Which types may participate stays guidance; argument order is enforced by the
   signature.** The first prompt merged the two into "hint, not a rule — when the text says
   otherwise, write what the text says", and the model applied that to argument order too.
   Order is not a claim about the world, it is the encoding convention of the key; the text
   never "says another direction", it only says a relation exists. For types, 0001 still
   holds: a hard gate loses data systematically, as `part_of` showed.

2. **The ancestor floor.** The ancestors of every retrieved class join the list, so `person`
   and `organization` are always available. This removed the type half of the violations and
   made entity types trustworthy: the people typed `researcher` became `person`.

3. **Correct direction at write time.** When the subject violates the domain and the object
   satisfies it, swap them by signature — the move `produced_by` → `produces` already made
   (#109), triggered by the signature instead of the wording. Types are read from the entity
   in the store, not from the extractor's `entity_type_of`, which only covers entities
   declared in the current chunk while the object usually already exists. That single
   difference took reversals from 10 to 0.

4. **Never silently.** Every swap leaves a `direction_corrected` trace. 0001 objected to
   automatic action driven by possibly wrong declarations; a traced action is a different
   thing. One of 29 swaps was wrong (`spatial`, whose schema.org domain is vague) — the
   built-in cost of trusting a declaration that may not apply to this pair.

5. **A relation that does not apply falls silent.** When swapping is also illegal, keeping
   the predicate would assert in the ontology's name something the ontology disagrees with
   (`OpenAI --affectedBy--> …` is a medical-test property; `competitor` belongs to
   `SportsEvent`). The predicate is dropped, subject, object, time and evidence stay, the
   model's word goes to `fact_evidence.proposed_predicate` and `fact_surface_predicate()`
   shows it (0010). All 179 facts pushed back to an empty predicate still surface their wording.

6. **Two side repairs.** A leading light verb is not a distinction: `has_funding` and
   `funding` merge, and over-merging is harmless because a collision voids the match. A clause
   is not an entity name: the 100-character guard sat only on the declared-entities path while
   undeclared subjects and objects went straight to `resolve()`; the criterion is now word
   count plus a finite verb, since a 57-character court name and a 65-character clause cannot
   be told apart by length. Longest name 111 → 57 characters, untyped entities 76 → 60.

## Dead ends

- **More prompt wording.** Three rounds moved the violation rate from 57% to 35%, all of it
  type errors; true reversals stayed flat (22.7% → 17.1% → 17.6%, within noise). The model
  sees the signature and does not follow it — English "X is an employee of Y" is too strong.
- **Automatic swapping before the floor.** Rejected earlier because entity types were
  unreliable (Musk typed `researcher`); once the floor landed the premise fell away.
- **A richer modeling language for reification.** `amount` (13), `target` (8) and
  `participant` (5) fail because schema.org models them through intermediate nodes (Action /
  LoanOrCredit / Offer) and we write flat binary relations. Expressiveness is not the blocker:
  `entities` + `facts` already form a property graph. The blocker is prompt rules 1 and 2 —
  canonical names from the text, every subject and object an entity — and a funding round has
  no name in the text, so the model would have to invent one. The model is right. Revisit when
  a query needs the qualifier itself ("companies whose 2024 Series A was led by General
  Catalyst"); unnamed nodes would also need their own naming and dedup.
- **Blaming the guard for orphans** (entities with no fact, 14.0% → 17.5%). The guard blocked
  single digits; the run was restarted mid-way. The real cause is the model declaring
  entities it never uses (courts, the SEC, Tesla HQ) — worth measuring separately.

The remaining cost of a pack is selection difficulty, not prompt length: many names are
generic with narrow meanings (`affected_by`, `competitor`, `uses_device`, `Researcher`) and
get picked by name or vector similarity. 0008 should carry this.

## Revisions

- 2026-09-02: the write-time guard could not stop later edits — a merge swaps the subject for
  an entity of another type and the fact "becomes" a violation (4 of the 6 residuals). Done in
  #190 / #196: `ontology::judge_direction` is shared by extraction and adoption (adoption was
  a second path writing predicates and had pushed the rate back to 12.3%); adoption that fits
  neither way leaves the predicate off, counted under `facts_left_off`; merge does not swap,
  it reports `signature` violations on moved facts, and R0 re-checks them so a later change of
  domain cannot hide. An unclassified entity is not a violation — "unknown" is not "does not fit".

## Open questions

- Two residual violations (`Stability AI Ltd`, `Colossus 2 data center`) were neither merged
  nor retyped; the cause is unknown.
- Pack import still lays out relations whose domain is a reified shell (`Action`, `Offer`,
  `LoanOrCredit`); shown to the model they can only produce violations.
