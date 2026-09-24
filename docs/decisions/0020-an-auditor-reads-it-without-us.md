# 0020 · An auditor reads it without us

- **Status**: implemented · `GET /api/v1/kbs/{id}/export?format=turtle|jsonld` streams the base as RDF; `rdf.rs` holds the mapping, `oxrdfio` the serialisers · SPARQL is still not here, and this record says why it can wait
- **Written**: 2026-09-05 (conventions in the [README](README.md))
- **Related**: [0001](0001-ontology-import-and-governance.md) kept the imported file verbatim and projected what today's consumers can use — this is the first consumer pointing the other way. [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) and [0019](0019-the-second-clock-can-be-rewound.md) are what makes the export worth reading: the intervals and the lineage are the content, not the triples

> `owl_import.rs` reads OWL in and five packs ship inside the binary. Nothing went the other way. The data an auditor asks for is all on disk — facts carry validity, evidence points at documents and versions, the ledger records who and when — and it could only leave in the shapes the REST API happened to expose.

## Who this is for

Not another Utopia. A regulator, an auditor, or the team's own graph tooling: someone who has a triple store and a question about a decision that was made, and who should not have to learn our JSON to answer it. That fixes what the export must carry — **the intervals and the lineage**, not just the current edges — and it fixes the failure mode: an export that reads as a clean, confident graph while dropping the fact that half of it was retracted in March is worse than no export.

## Identity

**Revision 2026-09-11 ([#601](https://github.com/deeplethe/utopia/pull/601),
[#603](https://github.com/deeplethe/utopia/issues/603)).** This record originally
described the RDF mapping without explicitly stating its compatibility boundary.
#601 added that clarification and structured MCP reads sharing the ledger UUIDs,
restored the premise links already described under Lineage, and documented the
remaining gaps: conflict/review state, chunk identity behind quotes, and historical
proof snapshots. The documented MCP field policy is now explicit too: removing or
renaming a documented `structuredContent` field requires a decision record; clients should
tolerate additional fields.

The RDF mapping is the supported external machine-readable read contract
(clarified in [#550](https://github.com/deeplethe/utopia/issues/550)). A breaking
change needs a decision record explaining why; the internal tables and UI API
shapes are not that boundary. MCP may return the same UUIDs alongside its prose
so a caller can join an agent result to this export. The user-facing contract is
documented in [Agents over MCP](../../web/src/docs/mcp.md#the-external-read-contract).

Classes and relations that came in from an import **keep the IRI they came with**. A base built from the schema.org pack exports `schema:Organization`, not a Utopia mint of it, so the file lines up with the vocabulary the reader already has. That is the payoff for having stored `iri` on `entity_types` and `relation_types` since the first import.

Everything else is minted under a base namespace: entities, documents, and one IRI per fact. Facts get their own IRI rather than blank nodes because a statement someone may have to cite has to be addressable, and because `supersedes` needs somewhere to point. A class or relation the ontology grew itself is minted from its **key** rather than its uuid — the key is what the base already uses as its identifier (`UNIQUE (kb_id, key)`, and it is what the extraction prompt and the API speak), so the file stays readable; a rename is the one event that moves such an IRI, and renaming a class is rarer than reading the export.

The default namespace is a URN — `urn:utopia:kb:{kb_id}:entity:{uuid}` — and `?base=https://…/` overrides it for a deployment that publishes at a known address. The default is stable rather than dereferenceable, which is the right way round: two exports of the same base a year apart must line up, and a self-hosted deployment does not know its own public URL. Minting from the request's `Host` header would have made identity depend on which proxy the exporter came through.

## Intervals ride on the statement, not the triple

A triple cannot say "until July 2024". The three ways to fix that:

| | Why not |
|---|---|
| RDF-star (`<< s p o >> :validFrom …`) | The natural shape, and the one most consumers still cannot parse. A file nobody can load is not an export |
| A named graph per fact | Legal in TriG and JSON-LD, absent from Turtle, and it turns a base into tens of thousands of graphs. Tooling that groups by graph becomes unusable |
| RDF 1.1 reification | Verbose, universally understood, and needs no feature flags |

Reification wins on the only criterion that matters here — the file has to open in whatever the reader already runs.

So every fact appears as an `rdf:Statement` carrying `schema:validFrom` / `schema:validThrough` (world time), `prov:generatedAtTime` / `prov:invalidatedAtTime` (record time), its confidence, and its evidence. **Standard terms wherever they exist**: PROV-O's `invalidatedAtTime` means exactly what our recording axis means, and reaching for a custom term there would hide a familiar concept behind a private name. Three things have no standard spelling and get a minted one: `confidence`, `derived`, and the model's original wording for a predicate the ontology never accepted.

Facts **currently held and currently valid** are additionally written as the plain triple. A consumer that ignores reification then gets a correct present-tense graph rather than an assertion that Zhang San still runs a project he handed over in 2024. Everything else — closed intervals, retracted rows, corrections — is in the file exactly once, as a statement, where the dates are impossible to miss.

## Lineage is PROV-O because that is what PROV-O is

**Revision 2026-09-25 ([#902](https://github.com/deeplethe/utopia/issues/902)).**
The rule a derivation is `prov:wasGeneratedBy` used to carry only an `rdfs:label`,
so a reader parsed a label to learn whether it came from an axiom or a business
rule, and for `inverse` and `sub_property` could not recover which predicate the
axiom was declared on without re-deriving the engine's convention from the exported
`owl:inverseOf` / `rdfs:subPropertyOf`. Three minted terms close that: every rule
resource is typed `utopia:AxiomRule` or `utopia:BusinessRule`; an axiom rule
carries `utopia:axiomKind` (the closed enum `transitive`, `symmetric`, `inverse`,
`sub_property`) and `utopia:declaredOn`, the IRI of the predicate the declaration
sits on, which for `inverse` and `sub_property` differs from the conclusion's
`rdf:predicate`. The label stays. Nothing about business-rule bodies is added, for
the reason under **What is not here**. Storage names stay out of the vocabulary.

Each statement is `prov:wasDerivedFrom` the documents its evidence chunks belong to, and carries the quoted sentence. Documents are `prov:Entity` with their title and the source key they arrived under. Derived facts ([0002](0002-reasoning-engine.md)) are `prov:wasGeneratedBy` the rule that produced them, with `prov:used` on each premise statement, so a reader can walk from a conclusion to the sentences underneath it without our API — which is the sentence in the README that this record exists to make true.

One of the five built-in packs is PROV-O, so a base that has it loaded already knows these terms.

## What is not here

- **Conflict/review state and chunk identity behind a quote.** Quotes and source
  documents are exported, but these are separate gaps; conflict state is tracked
  in [#564](https://github.com/deeplethe/utopia/issues/564).
- **Business-rule criteria.** A generating rule is exported as `prov:Activity`
  under `…:rule:{id}` with an `rdfs:label`; since the revision below it also says
  which family it belongs to, and an axiom rule says its kind and the predicate it
  is declared on. A business rule's conditions, operands and expressions are still
  not exported, and the reason is not effort: a business rule is edited in place,
  so `…:rule:{id}` cannot vouch for the definition an older conclusion was drawn
  under, and exporting today's threshold on it would tell an auditor something
  false about yesterday's conclusion. That waits for rule versioning, a storage
  and provenance change of its own ([#902](https://github.com/deeplethe/utopia/issues/902)).
- **Historical proof snapshots.** Premise links can be rewritten when a conclusion
  is reproved. The record-time lifetime survives; earlier versions of the proof
  do not (0019). RDF's `prov:used` edges identify premises, not their sequence.

- **A SPARQL endpoint.** The escape hatch over an in-memory Oxigraph projection was the original plan and stays a later cut. Someone who asked for "the reasoning behind this decision" wants a file they can keep; a query endpoint is the second thing they ask for, not the first.
- **Import of our own export.** The export is not a backup format, and reading it back would need entity resolution to be told "these IRIs are already resolved". Nothing stops it later; nothing depends on it now.
- **A button.** The API is the deliverable; where the download lives in the interface is a separate cut.
