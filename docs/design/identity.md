# Identity is decided on evidence

Records: [0041] (names and the plan), [0001] (name collisions), [0009] (untyped namesakes,
disjointness), [0016] B3, [0012] (a clause is not a name), [0019] (an entity's clock), [0025] to
[0028] for the adjudicator and governor (in [governance](governance.md)), [0044] d6 and #725 for
the proposed redesign.

## What it does today

**A name is a fact.** Every name a text uses for an entity is a value fact on the system attribute
`known_as` with its quote and both clocks; `canonical_name` stays the label and is one of the name
facts; `entities.aliases` is gone. A name is a value and never a node, so it adds no edge and no
count; `known_as` is neither functional nor inverse-functional, so a second name closes nothing and
opens no conflict [0041 d1, #670]. The extractor reports other names (`n` in the open contract) and
the server keeps one only when it occurs verbatim in its quote and the quote in the chunk; a string
another entity of the document already claims is dropped (`name_claimed_by_another`) [0041 d2]. A
described thing gets no name fact and is never recalled by name [#731].

**Resolution, one mention at a time, while extraction writes.** Recall has two channels. The first
is an exact match of the mention's name (and its generic-suffix-stripped keys) against the name
facts of entities of the same type. The second is the mention's name vector against the name
vectors of the base (`name_vectors`, one row per name fact, embedded after each document; migration
0080): the nearest few within the same type family at cosine 0.60 or above are *proposed* as a
`name_vector` pair for the adjudicator and never attached, so a short form or a name in another
script meets its entity through a question rather than a silent second entity [0041 d3 channel 2,
#709]. Such a pair says in the adjudicator's prompt that its names are similar, not the same
string, and a batch verdict of *same* on it is never applied directly: it takes the tool-using
second look first, whatever its confidence, and when that look cannot run (the daily loop budget
is spent, the model fails) the pair goes to a person as `second_look_unavailable` instead, with the
batch verdict left out of the verdict cache; the same rule holds with governance on. The identity
bench showed the batch step merging 张伟 into 财务部总监张伟 on name alone. Only the first channel decides anything: a context vector (`profile_embedding`, the running mean of chunk vectors) attaches at cosine
0.55 or above, makes a new entity below 0.35, and in between makes a new entity and a pair for the
adjudicator; two same-name candidates within a tie margin go to a person unless a candidate's object
name appears in the chunk [0041, #270, #331]. A name another entity of a compatible type already
holds queues a `shared_name` pair for the adjudicator, never a merge, and the card lists an entity's
non-canonical names as "also known as" [0041 d5 revised]. Containment pairs (`Holmes` inside
`Sherlock Holmes`) queue for names of four characters and more, capped per run; after a merge the
other pending pairs on the source are redirected to the target rather than closed
([pipeline](../pipeline.md)). Declared `disjointWith`, inherited down both hierarchies, keeps
same-name entities apart ahead of every heuristic; kin classes go to Review; `CONFUSABLE_TYPE_KEYS`
is the fallback when nothing is declared [0009, 0016 B3]. Untyped same-name entities may coexist
[0009].

**Merges are reversible on both clocks.** `entity_merges` records what moved with `created_at` and
`reverted_at`; `revert_merge` moves facts, name facts and qualifiers back; a read `as_of` a moment
before a merge shows two nodes [0019, 0041]. A merge is a person's action, the adjudicator's at 0.8,
or the governor's through its gate, and an automatic merge is held for a person when it would leave
the graph (a functional contradiction at one moment, a derivation, a cited answer) [0027]. Retypes
and renames are audited with before and after; a name collision on rename is a prompt to merge, not
a 409 [0001].

**Adjudication.** Twelve pairs per call with names, types, top facts, "also known as" and precedents
from the ledger; unsettled pairs get a second look with tools; the identity rules (a version, edition
or division tail is a different thing; a phrase containing a name is not the name; a list is not its
members; a dropped qualifier, a corporate suffix, a surname or an acronym is the same thing; a
document stays one thing through its amendments) are written once and read by both prompts, and two
are mechanical: `name_shape` and `type_family` [0025 d10, 0028]. Details in
[governance](governance.md).

**Bench.** `scripts/bench/identity.mjs`: a bilingual corpus with namesakes, abbreviations in both
arrival orders, a dated rename, two scripts, and look-alikes; scored as pairwise precision and
recall of same-entity, run forward and reversed, the difference reported. Cut 1 moved forward and
reverse F1 from 0.43 and 0.54 to 0.68 and 0.68 (one run each); re-measured on DeepSeek-V3, 0.61
and 0.44 to 0.53 over three runs [0041].

## Why

- **Identity keyed on the surface string fails two ways**: a namesake is one string for several
  things, an alias is several strings for one; a topic vector records what a passage is about, not
  who is in it; the outcome depended on arrival order [0041].
- **A name is a fact because it can be wrong, corrected and dated**, and the ledger already does that
  for everything else; a separate table would rebuild validity, provenance and revert [0041].
- **The server checks presence in the text, not a word list**; every list so far covered the
  phrasings its author thought of, and `name_shape` cannot see a Chinese name [0041, 0044 d8].
- **Keep apart when unsure**: a wrong merge mixes two entities' facts, and a revert does not recall
  the derivations, violations and answers built on it meanwhile [0001, 0027].
- **Two 张伟 of one type must be storable apart**, so the name index is plain and NULL types do not
  collide [0001, 0009].
- **A shared name goes to the adjudicator, never to a merge**, because reverse arrival order put the
  full name on a new entity and nothing paired the two [0041 d5].
- **A clause is not an entity name**: judged by word count and a finite verb, since a 57-character
  court name and a 65-character clause cannot be told apart by length [0012].

## Proposed and not built

- **Cuts 2 to 4 of 0041**: name vectors and shared distinctive neighbours as recall channels;
  evidence decides (a shared distinctive edge or an inverse-functional value with overlapping
  validity attaches; a functional conflict or declared disjointness rules out; a shared name says
  nothing; the context vector only breaks ties); mentions resolved after the response is parsed and
  `doc_cache` removed; re-evaluation when a name or edge arrives; `name_shape` and the suffix lists
  leave once the bench says the gate catches nothing evidence misses.
- **Identity across documents on profiles** [0044 d6]: entities merged within a document, profiled
  by names, classes, attributes, neighbours and time span; candidates from name facts and name
  vectors across scripts; deterministic scoring with cannot-links; only undecided pairs to the
  adjudicator, which sees two profiles; constrained clustering so A≈B and B≈C never merge A and C
  against evidence; a rename is one entity with names valid at different times; a role is not an
  entity.
- **#725 retires** write-time resolution (`resolve_mention`, the thresholds, `containment_reviews`,
  `resolve_type_drift`, `doc_cache`, the no-embedding fallback), the batch adjudicator and
  `resolution_verdicts`, and the word lists; removal waits until the design passes the identity
  bench and the Linked-Re-DocRED pairs.
- The entity panel's list of names with sources and validity; evidence lines on a Review card
  [0041].

## Open questions

- Whether a description reported as a name ("海探1项目") can be kept out without a word list [0041].
- Two untyped namesakes rely on profile similarity alone [0009].
- How much of identity deterministic evidence settles before the adjudicator is needed [0044].
- Cross-base precedent for namesakes [0025].
