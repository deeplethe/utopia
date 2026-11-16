# 0073 · A scenario pack is a vocabulary with its questions

- **Status**: In progress · accepted 2026-10-22 · decisions 1 to 3 and the first three packs built 2026-10-22 (`scenario_packs.rs`, `packs/scenarios/`, `CreateKbReq.scenario_packs`, `GET /scenario-packs`) · open: decision 4, the competency bench per pack
- **Written**: 2026-10-22 (conventions in the [README](README.md))
- **Related**: [0008](0008-ontology-packs-as-cold-start.md) ships standard vocabularies as the cold start and decided against bundles; [0061](0061-the-ontology-is-proposed-from-the-open-graph-and-judged-by-its-questions.md) makes competency questions the standard an ontology is judged by; [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) makes the ontology a view bound after extraction; [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) makes every element a contract a person signed; [0004](0004-language-and-localization.md) says a label is data and a description follows `ontology_lang`.

## Context

A base starts with one of three vocabularies: nothing, a standard pack (schema.org, W3C Org, PROV-O, FOAF, IOF Core), or an OWL file the person already has [0008]. None of these is a scenario. A pack gives types, not the predicates a corpus needs (81% of facts had no predicate after a pack, 0008); an OWL file presumes the modelling has been done elsewhere; and the empty start grows a vocabulary bottom-up from what the documents say, judged by questions the person writes afterwards [0061].

What a person who uploads company profiles, a contract chain or personnel records wants is the top-down start: the classes and properties that scenario is about, with definitions the aligner can read, units and datatypes already right, and the questions the base is meant to answer already written down. Measured on 2026-10-22 with a hand-written scenario (one class, six attributes, four documents), the modelling itself took minutes of someone who knew that an attribute needs a domain, that `注册资本` wants `number` in `万元` and that a definition is what the aligner reads; a person who does not know that builds an ontology that binds nothing, and reads the result as "extraction does not work".

0008 decided against bundles because the standard packs align pairwise and a bundle of them is guesswork. A scenario pack is not a bundle of standard packs: it is a small vocabulary written for one kind of corpus, in the corpus's own words, with the questions that corpus is read for.

## Decisions

**1. A scenario pack is a small vocabulary with its questions, shipped in the binary.** A pack is one JSON file under `crates/utopia-server/packs/scenarios/`, embedded like the standard packs so an air-gapped deployment has it. It carries: classes (key, label, description, parents), attributes (key, label, description, datatype, unit, functional, the class it hangs on), relations (key, label, description, domains, ranges, temporal, functional), and competency questions (the question, optionally the expected answer and the shape it needs). Labels and descriptions are written in both `zh` and `en`; installation picks the base's `ontology_lang` for the description and the label, since the label is data the documents will be matched against and a Chinese corpus writes `企业`, not `Company` [0004]. Keys are the same in both languages and are the API's tokens.

**2. A scenario pack installs through the editor's path, not the OWL importer.** Classes and properties are created with `ontology::create_entity_type` and `create_relation_type`, the same calls the ontology page makes, so every element is a contract in the base's own tables with no IRI [0012]: a pack element is editable and deletable like a typed one, and the export writes it as the base's own vocabulary. Questions are created `accepted` with `origin = 'person'` and the creating person as author: ticking the pack is the person's statement of what the base is for [0061 decision 1]. Installation queues `embed_ontology` so the elements have vectors before the first alignment, as the editor and the importer do. A pack installs only at creation (`CreateKbReq.scenario_packs`, beside `ontology_packs`), for the reason `install_packs` gives: a new base has no facts, so nothing needs reconciling.

**3. Scenario packs are additive and compose with standard packs by key.** A pack's keys are chosen not to collide with each other; a key that already exists in the base (from another scenario pack or a standard pack installed first) is kept as it is and the pack's element is skipped and reported, never merged or overwritten. Domains and ranges inside a pack name the pack's own class keys; a pack may name a class another pack defines only by the same key, and an unresolved key is reported and the declaration dropped rather than the install refused. No undo, as for 0008: knowledge vetoes deleting an element with facts, and the bulk view of empty elements is the answer to a wrong pack.

**4. A pack ships with the questions it is measured by.** The competency bench of 0061 runs each pack's questions over a corpus of its kind, and the first three packs are measured before the record is marked implemented. The two numbers of 0061 decision 5 are the pack's numbers.

**5. Not in this record.** No regression cases in a pack: a case names a statement, and a new base has none [0061 cut 2]. No implication rules: a rule is keyed by a signature the documents have not written yet. No per-scenario view over a base's ontology: a base has one ontology, and two scenario packs installed together are one vocabulary. No scenario packs in the ontology page's import menu: that is the open question below.

## The first three packs

- `enterprise-profile` 企业档案: 企业, 人员, 产品, 地点; 注册资本 (number, 万元), 成立时间 (date), 员工人数 (number), 营业收入 (number, 万元), 净利润 (number, 万元), 法定代表人 (text), 注册地址 (text), 统一社会信用代码 (text); 控股 (企业 → 企业), 任职于 (人员 → 企业, state), 主要产品 (企业 → 产品), 总部位于 (企业 → 地点). Questions: a company's registered capital, revenue in a year, legal representative, holding chain.
- `contract-chain` 合同链: 合同, 当事方, 条款, 补充协议; 签订日期, 合同金额 (number, 万元), 有效期起止, 付款期限; 甲方, 乙方, 修订 (补充协议 → 合同), 约定 (合同 → 条款). Questions: which terms applied on a date, who the parties were, which amendment changed a term.
- `personnel-records` 人事档案: 人员, 部门, 岗位; 入职日期, 离职日期, 职级, 工号; 任职于 (人员 → 部门, state), 担任 (人员 → 岗位, state), 汇报给 (人员 → 人员, state), 隶属 (部门 → 部门). Questions: who held a post in a period, who reported to whom, what the base believed about a holder at a time.

## What it costs

- One JSON file per pack and a loader of a few hundred lines; no schema change: packs write the tables the editor writes.
- A pack's elements are the base's own, so a pack update does not reach bases that installed the old one. That is the price of decision 2 and the same as a typed element.
- Descriptions are load-bearing (0061): a pack's definitions are what the aligner reads, so a pack is written like an ontology, not like a form.

## Dead ends

- **A scenario as a selection of standard-pack elements.** schema.org has `Organization` but not `注册资本`; the value of a scenario is the properties, and the standard packs do not have them (0008 measured 81% unbound).
- **A scenario as an extraction prompt.** Extraction does not read the ontology [0044]; a scenario that changed the prompt would bind at write time, which 0044 measured at 4 to 9% not stated against 0 of 333.
- **Installing a scenario pack into an existing base.** `install_packs` is written for an empty base; a base with facts needs the import's baseline and reconciliation, which is a second cut if it is wanted.

## Open questions

- Whether a pack should be installable later from the ontology page, with the import path's reconciliation.
- Whether a pack's questions should carry expected answers for a sample corpus shipped with the pack, so the competency bench runs without a corpus of the person's own.
- Where the line between a scenario pack and a standard pack falls for industry vocabularies (FIBO, a pharma pack): size, or whether it is written in the corpus's words.
