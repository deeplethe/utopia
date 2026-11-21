# Source-mention references: an internal protocol draft for review

Status: proposed, neither accepted nor implemented. This draft is for discussion under #1454 / #1136 before an ADR. It is not a new version of the public statements protocol. See the [source-mention reference proposal](source-mention-reference-proposal.md) for the reasoning.

## Recommended decisions

1. **A model response ref identifies a mention; the server still resolves the entity.** The same surface text may have refs m0 and m1. Different refs do not establish a cannot-link constraint. The server validates and deduplicates an occurrence using `(chunk_id, doc_version, text_hash, start_char, end_char)`; it does not accept model-supplied database entity IDs or trust model-supplied offsets.
2. **Validate references before writing the graph.** The server supplies a source marker and fragments f0, f1; the model copies text from the current fragment. The fragment, quote and mention must each locate uniquely. The server computes coordinates in the existing Unicode scalar units, retaining a mapping from collapsed whitespace to the original text. Every reference used by a chunk must validate before that chunk can write to the graph.
3. **Key the current response's name map by ref and the document's known list by entity ID.** Subjects and entity objects in s, owners in n, and entity-valued qualifiers all use refs. The known list may contain several records with the same name, distinguished by temporary server labels k0/k1. These are candidate hints only. The server still checks candidates, types and human constraints; a model's candidate hint does not authorize binding.
4. **Keep the legacy entry point separate; do not downgrade the new one.** Without the new model mode, use the existing e/s/n/t format. With it enabled, accept only internal responses with version=2. Invalid references cannot fall back to name-based mapping. Statements pushes retain their protocol, including null quotes, identity sources and memories awaiting confirmation; the existing entry point already rejects fields outside its protocol.

## Minimal request and response

Server fragments come from the current immutable chunk. The marker binds the chunk ID, version, full-text hash and fragment layout. Changing fragment boundaries must invalidate the old marker even when the text is unchanged. The model returns only an opaque digest marker, not database IDs or positions. S below is a readability shorthand, not an actual validation token. Fragment text is sent once; the model does not return character offsets.

The Chinese source text below is retained verbatim from the regression corpus: the finance director Zhang Wei receives the management award, and the engineer Zhang Wei receives the technical award.

```text
source: S
f0: 财务部总监张伟获得年度管理奖。
f1: 工程部工程师张伟获得年度技术奖。
```

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

An entity object uses `"object":"m1"`, mutually exclusive with value. An entity-valued qualifier uses `{"instrument":{"entity_ref":"m2"}}`; a literal qualifier uses `{"amount":{"value":"CNY 50,000"}}`. An alias uses `{"owner":"m0","name":"another name","fragment":"f0","quote":"source text containing this alias"}`. References must fall within that statement's or alias's own quote. Class and time terms retain their source wording. Their complete wire adaptation and the existing t semantics must be included in the ADR before implementation; fields absent from the prototype are not already migrated.

When one sentence contains two people called Zhang Wei, each mention can copy a locating span containing only its own occurrence, such as “财务部张伟向” (finance's Zhang Wei addressing someone) and “工程部张伟致谢” (engineering's Zhang Wei being thanked). The statement still retains its full quote. Repeated whole sentences use their respective server fragment IDs. Reject the chunk if a reference cannot locate uniquely; never choose the first match.

## Location and identity context are separate

Fragment boundaries are a locating aid. The offline prototype splits at punctuation, treats a single newline as whitespace and does not split decimal points. Abbreviations or other boundary cases can use a virtual whole locator over the existing chunk, without sending the full text again. Text from whole does not automatically become identity context for the resolver.

A valid location proves that the text exists there, not that the model assigned the correct subject, object or identity. Candidate comparisons still require evidence owned by the current mention; shared or conflicting quotes do not authorize a changed decision. The new protocol does not remove the legacy object-only protection. Entity objects in the new format can represent independent mentions, but when their locating text may become resolution context is an explicit review question, not an implicit expansion of the evidence scope.

If one person is known to hold both roles, m0/m1 may resolve to the same ID. Do not add the first resolution result to the second mention's exclusions. Actual caller exclusions, explicitly incompatible types and prior different decisions remain effective. Two roles, refs or fragments alone do not prove two people.

## Failure, compatibility and cold starts

| Input | Proposed outcome |
|---|---|
| Wrong chunk, stale version, changed hash, fabricated/missing/ambiguous location, dangling/duplicate ref, or truncated new-format response | Fail before graph writes. Use existing CHUNK_UNEXTRACTED, unextracted and graph_error signals, and report unfinished work and coverage loss. Valid fragments alone do not make the chunk complete. |
| Valid sources with clear existing candidates for both mentions | Resolve each mention, allowing the same entity ID. Validate source location separately from identity. |
| Valid sources with repeated names, but no basis for distinguishing identities in a cold start | Do not derive entity count from ref count. This slice leaves the chunk unfinished with a reason. Do not add an unresolved-entity pool or permanent kept state, or claim a fix by writing fewer facts. |
| Legacy model or statements request | Retain the existing entry point, null quotes and public field semantics. |
| Memory logs requiring confirmation, identity-name sources, human decisions or reverts | Retain existing authorization and execution protections. Valid source coordinates grant no new authorization. |

Existing unfinished signals retain the chunk text and failure reason, but production defaults do not guarantee retention of every raw model response. Experiments separately record complete replies and hashes. Strict rejection of incomplete references may reduce coverage and must be reported separately; it cannot inherit successful counts from the legacy parser's tolerant recovery. Retry through the existing mechanism, without adding model-call rounds or persisted states.

## Verified and outstanding

- The reference prototype covers 33 named scenarios in both input orders: 66 rows including PDF whitespace, Unicode, decimals, abbreviations, subject/object/qualifier/alias references, fragment-layout changes and expected rejections. This is not acceptance of a production parser.
- Three valid examples were passed to the unchanged production resolver: separate identities, one person holding two roles, and same-name subject/object mentions in one sentence. Across seed and mention orders, all 12 rows selected the expected existing IDs and created no entities. Four same-person controls retaining the old response_claims exclusions incorrectly split the person and each created one entity.
- The prototype validator and production resolver actually ran. New production extraction/apply and complete fact writes did not, so these results do not establish production acceptance of A02/A03. The 19 existing statements-related tests passed with database/PDF checks required.

The next production slice must cover class/time fields, known, multiple chunks and re-extraction, warm and cold starts, the entity-object evidence boundary, and complete database acceptance for A01–A11. Only then run a same-session 16-KB comparison. Review is requested on the decisions above, cold-start failure policy and entity-object evidence boundary. Do not switch production modes or close #1136 before agreement and a landed ADR.
