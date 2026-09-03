# 0004 · Language follows the reader of each text

- **Status**: Built · UI strings in `web/src/i18n/`; user-reachable server errors carry a `code` (`AppError::Invalid`, 81 sites; 23 `Validation` guards stay English); `description` follows `knowledge_bases.ontology_lang`; LLM output for people takes `locale` from the request; extracted data stays verbatim; the UI does not guess the browser language yet.
- **Written**: 2026-08-29 · condensed into English 2026-09-03
- **Related**: [0001](0001-ontology-import-and-governance.md) made `description` load-bearing; [0003](0003-ontology-growth-loop.md) separated `reason` from `description`; [0008](0008-ontology-packs-as-cold-start.md) supplies the English packs a Chinese knowledge base starts from.

## The problem

One language switch cannot serve five kinds of text with different readers and sources:

| Text | Reader | Source |
|---|---|---|
| UI strings ("Import an ontology") | people | the i18n bundle |
| Server errors ("Password must be at least 8 characters") | people | Rust |
| Ontology `label` / `description` | the model reads `description` | packs, users, OWL import |
| LLM text generated on the spot (a suggested `reason`) | people | the model, on demand |
| Extracted data (`张三`, `surface_predicate` "runs on") | people and the model | the document, verbatim |

The last row is not copy. It is a quotation.

## Decisions

1. **UI strings are a per-person client setting**, like dark mode: a foreign colleague in a Chinese company should not need an administrator to see English. Kept in `localStorage` via `makeStore`, resolved in one function that everything else consumes. The Chinese bundle is `const zh: typeof S = { … }`: a missing key fails compilation, where `Partial` with fallback would leave a silently English line.
2. **Server errors become keys.** With the locale in the client, any string left in Rust is permanently untranslatable. Errors a user can hit are `AppError::Invalid` with a `code` for the frontend `err.*` table, a machine `detail`, and an English `message` for MCP, CLI and logs. API-contract guards (`role must be admin, editor or viewer`) stay English `Validation`: their reader is a developer. Hence a hard rule: **the server produces no display text**.
3. **Ontology `description` follows the corpus.** It goes verbatim into the extraction prompt beside the documents, and Chinese UI with English class descriptions is common. So it is a knowledge-base property, `knowledge_bases.ontology_lang`, whose deployment default is named "default ontology language for new knowledge bases"; call it "system language" and someone will hang the UI language on it.
4. **`label` is data and follows no switch.** A class a Chinese team named `人物` reads `人物` on the English UI too: it is their concept. `key` is never translated; it is the model's token and the lookup key of `type_ids`. The prompt lists `key` + `description`, `label` only when the description is empty: `Person` beside `person` adds nothing and mixes scripts.
5. **LLM text for people takes `locale` from the request.** `reason` is generated when someone clicks "Suggest with AI", so the caller states the language; `label`/`description` in the same call follow `kb.ontology_lang`. `lang_name()` sends `Chinese` rather than `zh`; the cold-start path passes `en`.
6. **Extracted data stays verbatim.** An entity is `张三` because paragraph 3 says so; translating it breaks the evidence match, splits one person across Chinese and English documents, and empties `surface_predicate`. Provenance has no switch.

## Dead ends

- **UI language as a deployment-level admin setting**, "like the concurrency limit". That limit describes the deployment; the UI language describes the reader. A setting lives with whatever it describes.
- **A Chinese rewrite of the built-in ontology** (9 classes, 14 relations, negative examples included) lost its object when seeding was removed (#125, #128, [0009](0009-no-type-is-a-type.md), [0010](0010-no-relation-is-no-relation.md)). A Chinese knowledge base now starts from an all-English pack; `ontology_lang = zh` affects only terms the LLM proposes later. What remains is an imported vocabulary or packs with Chinese labels ([0008](0008-ontology-packs-as-cold-start.md), open question).

## Revisions

- 2026-09-02: "guess `navigator.language` once" was never shipped, deliberately: `detect()` reads `localStorage` and otherwise returns `"en"`, because the Chinese bundle trails the English one and untranslated lines fall back silently. The line goes back in when the bundle catches up.
- 2026-09-02: since #112 `suggest` writes `ontology_proposals` ([0003](0003-ontology-growth-loop.md)); the language conclusion is unchanged.

## Open questions

- **Bundle drift**: `typeof S` catches a missing key but not "the English wording changed and the Chinese did not". Edit both together.
- **Mixed-language corpora in one knowledge base**: one language per KB; design it when it happens.
