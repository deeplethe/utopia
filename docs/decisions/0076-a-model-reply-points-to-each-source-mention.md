# 0076 · A model reply points to each source mention

- **Status**: Proposed · 2026-11-24 · #1454 · open: protocol agreement, production implementation and acceptance
- **Written**: 2026-11-24 (conventions in the [README](README.md))
- **Related**: [0041](0041-a-name-is-a-claim-about-an-entity.md), [0054](0054-a-source-may-push-statements-in-the-open-contract.md), [#1136](https://github.com/deeplethe/utopia/issues/1136), [#1193](https://github.com/deeplethe/utopia/issues/1193), [#709](https://github.com/deeplethe/utopia/issues/709)

## Why

The awards document in the identity benchmark names two people called 张伟. The finance director receives the management award. The engineer receives the technical award.

> 财务部总监张伟获得年度管理奖。工程部工程师张伟获得年度技术奖。

Four recorded replies from the 2026-10-22 comparison declared one 张伟 and used that same subject string for both awards. The production parser kept one subject on the recorded base. A control with two entries of the same name in `e` still kept one entry. A separate control in which one person held both roles produced the same shape. The complete corpus supplies the identity labels for the awards case. The job titles alone are insufficient.

Today `parse_open_response` deduplicates entity names by `name_key`, and `apply_open_extraction` uses name-keyed maps for the current reply and later chunks. This can lose the distinction between occurrences before identity adjudication begins. `resolve_handle` also excludes same-name entities already claimed in the reply. Keeping that exclusion for every repeated mention would split the dual-role control. [Namesake issue #1136](https://github.com/deeplethe/utopia/issues/1136)

## Proposed decisions

1. **Give each source occurrence a reference local to the response.** A mention carries a surface string, a server-issued fragment label and copied text that locates the occurrence. Statements reference their subjects, entity objects and entity-valued qualifiers by mention ref. Aliases reference their owner. An entity object and a literal value are mutually exclusive. Each use must locate its reference within the statement's or alias's own quote. Distinct refs can resolve to the same entity, as in the dual-role control.
2. **Validate the chunk reply before graph writes.** An opaque server marker binds the chunk, document version, full text hash and fragment layout. The server uses the current fragment to find the copied locator and the locator to find the mention. Both matches must be unique. It computes offsets in Unicode scalar units and maps normalized whitespace back to the original text. The occurrence key is the chunk, version, text hash and verified character span. Database IDs and offsets supplied by the model carry no authority.
3. **Resolve occurrences with the existing identity safeguards.** Index mentions by ref and retain known candidates by entity ID across chunks. The model can receive temporary candidate labels such as `k0` and `k1` for records with the same name. These are hints. Candidate selection, type checks, identity evidence and human decisions remain server checks. A prior binding in the current reply does not by itself exclude that entity. Explicit incompatibilities and prior human decisions remain effective.
4. **Check location and identity evidence separately.** The locator establishes where text appears. The resolver checks whether the evidence belongs to the mention and supports its binding. Shared or contradictory text cannot establish an exclusive identity on its own. An object ref locates an occurrence independently. Treating its locator as identity context requires a decision about evidence ownership before implementation.
5. **Route strict validation through a versioned internal model mode.** The existing model route keeps `e/s/n/t`. The new route requires `version: 2` and the matching source marker. A stale marker, duplicate or dangling ref, fabricated or ambiguous locator, or incomplete response leaves the chunk unfinished before graph writes. Use the existing failure and retry paths and report the reason and coverage loss. A cold start whose identities cannot be represented faithfully also leaves the chunk unfinished. The public statements push contract in [0054](0054-a-source-may-push-statements-in-the-open-contract.md) keeps its field semantics and null-quote handling. Identity sources and memories awaiting confirmation retain their existing safeguards.

Fragment labels distinguish repeated identical sentences. Within one sentence, short locators such as `财务部张伟向` and `工程部张伟致谢` can distinguish same-name occurrences while the statement cites the full sentence. A virtual locator over the existing chunk can handle awkward fragment boundaries when both matches are still unique. Layout changes invalidate the marker. The choice of locator does not widen identity evidence.

### Example

The server sends each fragment's text once. This abbreviated reply shows the two statement owners and uses `S` as a stand-in for the source marker.

```json
{
  "version": 2,
  "source": "S",
  "mentions": [
    {"ref":"m0","surface":"张伟","named":true,"fragment":"f0","quote":"财务部总监张伟获得年度管理奖。"},
    {"ref":"m1","surface":"张伟","named":true,"fragment":"f1","quote":"工程部工程师张伟获得年度技术奖。"}
  ],
  "statements": [
    {"subject":"m0","phrase":"获得","value":"年度管理奖","fragment":"f0","quote":"财务部总监张伟获得年度管理奖。"},
    {"subject":"m1","phrase":"获得","value":"年度技术奖","fragment":"f1","quote":"工程部工程师张伟获得年度技术奖。"}
  ],
  "names": []
}
```

## Alternatives tried

- **Use the full description as the entity name.** This fits the legacy response shape. Identical text can still refer to different occurrences, and the description can turn a role into a name.
- **Locate a copied quote across the chunk.** This avoids trusting model offsets. The offline control correctly rejected ambiguous locations, including repeated sentences and same-name mentions in one sentence. Fragment labels can distinguish repeated sentences when the model points to the intended occurrence.
- **Assume different refs mean different entities.** Four dual-role controls reused the existing `response_claims` exclusion. Each falsely split one person and created an extra entity. The ref count is therefore insufficient as an identity constraint.

## Evidence and acceptance

The reference prototype exercised 33 scenarios in both orders, including Unicode, PDF whitespace, repeated text, stale layouts and invalid references. The existing resolver selected the expected pre-existing IDs in 12 rows covering namesakes, a dual-role person and a same-name subject and object. Those rows exercised location and resolution with known candidates. The historical awards replies had zero or one matching holder at the affected states. Cold-start behaviour and production fact writes still need acceptance.

Before implementation, agree on the entity-object evidence rule, class and time field encoding, known-candidate representation and cold-start coverage trade-off. Verify the production path from parsing through graph writes in both orders for the two-person and dual-role cases. Assert stored fact owners and their evidence, chunk-level failure before writes, and legacy and statements-push compatibility. Cover repeated chunks and re-extraction, aliases and qualifiers, stale source markers, and human and identity-source safeguards.

Then compare with the same model and unchanged identity corpus and scorer in one session across 16 fresh knowledge bases. Record false merges, false splits, missing anchors, unfinished chunks, entities, pending reviews and call cost. Measure whether the model represents both subjects separately from whether the server resolves them correctly.

[Identity merge policy #1193](https://github.com/deeplethe/utopia/issues/1193) and [cross-script recall #709](https://github.com/deeplethe/utopia/issues/709) track their own acceptance. The [original proposal revision](https://github.com/virtue039/utopia/blob/25b678c2025c8d8b138583edf914e5e1b72b681e/docs/design/source-mention-reference-proposal.md) retains the cross-script experiments.
