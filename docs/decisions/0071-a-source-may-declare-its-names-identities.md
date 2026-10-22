# 0071 · A source may declare its names identities

- **Status**: Proposed · 2026-10-19 · [#1293](https://github.com/deeplethe/utopia/issues/1293) · open: all seven decisions
- **Written**: 2026-10-19 (conventions in the [README](README.md))
- **Related**: [0054](0054-a-source-may-push-statements-in-the-open-contract.md) decision 3 sends pushed statements down the same resolution path as prose, and this record narrows that for one kind of source; [0041](0041-a-name-is-a-claim-about-an-entity.md) owns name facts and name-vector recall; [#1248](https://github.com/deeplethe/utopia/issues/1248) meets the same problem from the merge side and is decided here as decision 7; [#1296](https://github.com/deeplethe/utopia/issues/1296) measured what the recall costs; [#1308](https://github.com/deeplethe/utopia/pull/1308) is the disjointness fix that stands on its own

> A backend mirrors a repository into a base and pushes `owner/repo:src/a.ts#parseDate`, `owner/repo:src/a.test.ts::adds two`, `owner/repo:specs/x/spec.md#s0003`. Each name is an identity the client already knows to be unique. Every one still goes through write-time resolution. Because they share the `owner/repo:` prefix, their name vectors sit close together, and the duplicates queue fills with pairs that can never be the same thing.

## The problem

0054 decision 3 is right for most pushes. A client that pushes human-style names, like `cup-7` and `kitchen table`, gains from name-vector recall exactly as prose does. What is true for some producers is narrower: *this source's* names are identities. For such a source, resolution today does three things it already knows the answer to:

- **Name vectors** (`propose_by_name_vector`). Up to eight `name_vector|` pairs per mention, plus one `name_vectors::nearest` query per named entity per pushed document, even after an exact match (#1296).
- **The single-candidate split** (`resolve_by_name`, cosine below `SIM_ATTACH`). The same file named in two payloads with different context becomes two entities with one name.
- **Being a target.** Its names' vectors are near-neighbours for every other mention in the base, so prose mentions propose pairs against them too.

## Decisions

**1. The switch lives on the source.**

A `statements` source whose config says `{"names": "identity"}`, next to the existing `{"extract": false}`, declares that every name it pushes identifies one thing. The default is unchanged, and every other source keeps 0054 decision 3 as it stands. The producer is the one that knows its names are identities, and one base can hold an identity source beside prose sources, so the switch is per source rather than per base. #1248 decides the merge side with the same flag.

**2. An identity name attaches to its exact name, whatever the profile.**

A named entity from an identity source attaches to the entity whose name equals it case-insensitively, in the declared class or with no class, and is created only when there is none. The profile similarity is not consulted, so the single-candidate split never happens for an identity source: the same name in two payloads is one entity. When the exact name already has two holders, which should be rare for identity names, the mention takes today's review path. Choosing between them would need a provenance lookup that this record does not add.

**3. A name that exists only under another class is created in the declared class.**

The source's class is part of its identity claim, so the entity is created in that class. The type-drift review stays, unless the two classes are declared disjoint (0016 B3): an exact name under another class is most likely a duplicate whose class was guessed, and it deserves one look.

**4. An identity name has no name vector.**

A mention from an identity source runs no `name_vectors::nearest` query and proposes no `name_vector|` pair. Its name facts get no vector either: `name_vectors::pending` skips a name fact that has evidence from an identity source (through `fact_evidence.document_id` to the document's source). So identity entities neither propose pairs nor become targets of one. A prose mention of the exact name still attaches by exact match (channel 1 of 0041 decision 3), so nothing that an exact name finds is lost.

**5. Identity evidence wins.**

One name fact can carry evidence from both an identity source and a prose document, because facts dedupe on subject, predicate and object. Such a fact gets no vector. The identity source has vouched for the name, and a prose mention still reaches it by exact match. This changes recall for prose: a prose mention of a *variant* of that name (an abbreviation, another script) no longer finds the entity through its vector, and the code PR covers that with the test `a_name_with_identity_evidence_gets_no_vector_even_when_prose_names_it_too`.

**6. Switching the flag.**

- **On:** the update that sets the flag also deletes the stored vectors of name facts that now have identity evidence, in the same transaction. Leaving them in place and skipping them would put an evidence join into every `nearest` query, which is the per-mention cost #1296 measured.
- **Off:** nothing is deleted. Those name facts become pending again, and the next extraction in the base backfills their vectors through `pending`, as it does for any name whose vector is missing. Recall for them returns from then on. Pairs that were never proposed while the flag was on are not proposed in retrospect.

**7. A `same` verdict that touches an identity entity goes to a person.**

*Revised 2026-10-22 (#1248):* decision 1 said #1248 decides the merge side with the same flag; this is that decision. An identity entity is one with a name fact that has evidence from an identity source, the same join decision 4 uses. When the adjudicator returns `same` at or above `AUTO_CONF` for a pair where either side is an identity entity, the pair is escalated to human review with the reason `escalate_identity` instead of being merged. This holds on both paths that apply a verdict: `apply_verdict` in `adjudication.rs` with governance off, the second look included (#889), and `apply` in `governance.rs` with governance on. It reuses the execution gate's escalation (0027), so a person sees why the pair is waiting, not "the adjudicator was unsure". A `kept` verdict still closes the pair automatically, which keeps the queue short. A merge a person makes is unchanged.

Why a person and not a stricter threshold: an identity source has already said what its names are. Decisions 2 to 4 remove the pairs that name vectors and the single-candidate split would raise, so what is left are containment and type-drift pairs and the cross-source duplicate (one thing pushed under two names by two producers). Those are exactly the cases where the model's `same` is a guess about a claim the producer made, and a wrong merge of identities is what #1248 measured: on a small local model, 40 automatic merges with governance off, nearly all wrong (a method into its file, two unrelated types). A person sees few of them because the vectors are off. Raising `AUTO_CONF` for these pairs would still let a confident wrong verdict through, and a per-base switch is a dead end (below).

## Not doing

- **Containment for identity names.** `containment_reviews` still runs for them. #1104's next cut rewrites that predicate, and whether an identity source skips it is a separate decision after that.
- **The single-candidate split for other sources.** Decision 2 removes it for identity sources only. Prose keeps today's split.

## Dead ends

- **Every `statements` push is an identity claim.** Too broad: 0054 sends pushes through the same path on purpose, and human-style names find real variants through their vectors.
- **Skip the vector channel after any single exact match.** That is #1296 item 1. Applied to every mention it changes prose resolution, and `the_same_literal_name_is_not_proposed_twice` asserts the opposite.
- **A per-base switch.** One base can mix an identity source with prose sources, and only the producer knows which names are identities. #1248 first asked for `auto_merge: false` per base; decision 7 keeps the merge gate on the same per-source flag instead.
- **Closing a `same` verdict between two identity names as kept.** "Every name identifies one thing" does not say two names never identify the same thing: two producers can push one service under two names. A person decides those.

## Tests the code PR adds

- `an_identity_source_adds_no_name_vectors_and_asks_no_neighbours`: a push from an identity source leaves no `name_vector|` rows and runs no `nearest` query.
- `a_name_with_identity_evidence_gets_no_vector_even_when_prose_names_it_too` (decision 5).
- Turning the flag on deletes the identity names' vectors; turning it off makes them pending again (decision 6).
- `a_same_verdict_on_an_identity_entity_waits_for_a_person`: with governance off and with it on, a `same` verdict at or above `AUTO_CONF` on a pair with one identity side leaves both entities in place and an open review with `escalate_identity`; a `kept` verdict on the same pair still closes it (decision 7).
