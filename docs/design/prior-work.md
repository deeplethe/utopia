# What prior work taught us

The three layers of [0044] and the time resolution of [0045] were not invented here. Each stands on
a line of research that is fifteen to forty years old, and each of those lines has already met the
failures we meet on our own corpora. This file places every layer in that literature and lists the
pitfalls we took from it: who found it, what we do about it, and what is still missing. It changes
when a design file changes for a reason the literature had already given, or when a new pitfall is
taken. A paper is cited as [Fader 2011]; the list at the end gives venue, year and a link for every
one, and every entry was checked against its publisher page before it was written here.

## Where each layer stands

**The open graph is extractive Open Information Extraction.** A statement in the document's own
words, with the relation phrase as written and no schema, is the programme Banko et al. opened
[Banko 2007] and the Etzioni group carried through ReVerb [Fader 2011] and OLLIE [Mausam 2012];
ClausIE [Del Corro 2013], Stanford OpenIE [Angeli 2015], the supervised
turn [Stanovsky 2018] and OpenIE6 [Kolluru 2020] are the same programme with better parsers, and
three surveys cover it to 2024 [Niklaus 2018, Zhou 2022, Liu 2024]. Pei et al. draw the line we sit
on: extractive OpenIE keeps surface forms, abstractive OpenIE may infer [Pei 2023]. For Chinese the
reference points are CORE [Tseng 2014], ZORE [Qiu 2014], the SAOKE corpus behind Logician, whose
annotation scheme has a class of descriptive facts that matches our described things [Sun 2018],
and Title2Event, the largest manually annotated set of Chinese open triples [Deng 2022]. Our
departures from that tradition are three: a language model reads the chunk under a JSON contract
instead of a parser; every statement carries its own verbatim quote located by character offsets;
the run is at temperature 0 with a judge that counts what the text did not say
([extraction](extraction.md)). Two recent designs converge on the same shape from the other side:
TRACE-KG keeps a raw and a canonical predicate, qualifiers and a justification excerpt on every
relation [Abolhasani 2026], and Han and Lai argue for natural-language relations over a minimal
structural backbone [Han 2026].

**Qualifiers beside the edge are MinIE's attributes and Wikidata's qualifiers.** OLLIE first
separated attribution and condition from the triple [Mausam 2012]; MinIE moved polarity, modality,
attribution and quantity out of the relation phrase into annotations [Gashteovski 2017]; NestIE kept
nested propositions as such [Bhutani 2016]; DisSim's split of a sentence into core and context
linked by a rhetorical relation is the same cut made by rule [Niklaus 2019]. In the ledger the idea
is Wikidata's qualifier on a statement [Vrandečić 2014] and the hyper-relational fact of HyperRED,
whose 44 qualifier types are the comparison point for our role words [Chia 2022]; Text2NKG lists the
four incompatible ways an n-ary fact is serialised [Luo 2024], and ours is the hyper-relational one.
`statement_qualifiers` keyed by the document's role word is that model with the role word left as
written [0037, 0044].

**The ontology as a view, with alignment later, is open-KB canonicalization plus pay-as-you-go
integration.** Canonicalizing the noun phrases and relation phrases of an open knowledge base after
extraction is the problem Galárraga et al. posed [Galárraga 2014] and CESI [Vashishth 2018a], CUVA
[Dash 2021], JOCL [Liu 2021] and CMVC [Shen 2022] took up; Yang and Curry survey that line and find
relation-phrase canonicalization its least solved part [Yang 2024]. Mapping open extractions onto a
fixed schema was done for domain relations [Soderland 2010], for KBP slots [Soderland 2013, Angeli
2015], for DBpedia [Dutta 2015, Gashteovski 2020] and for ConceptNet [Romero 2023]. The LLM-era
pipeline closest to [0044] is EDC, which extracts open triples, writes a definition for each
extracted relation and then lets the model select a schema element or reject them all [Zhang 2024];
KGGen [Mo 2025], iText2KG [Lairgi 2024], AutoSchemaKG [Bai 2025], SAC-KG [Chen 2024b], Docs2KG
[Sun 2025] and ODKE+ [Khorshidi 2025] are the same order of operations at other scales, and Bian's
survey names the schema-based and schema-free paradigms they sit between [Bian 2025]. The stance
that data is stored first and semantics added as it pays for itself is the dataspaces programme
[Franklin 2005, Halevy 2006, Madhavan 2007]; an ontology that is a view over stored data through
mappings is ontology-based data access [Poggi 2008, Xiao 2018]. The knowledge-graph surveys treat an
emergent schema as a normal third mode beside a prescribed one [Hogan 2021, Weikum 2021].

**A kind word bound to a class is ultra-fine entity typing, decided by a judge that has been shown
its own bias.** Choi et al. defined types as the free noun phrases a text uses, with head words as
the supervision [Choi 2018]; our kind word is that phrase kept verbatim, and the binding is its
mapping to the ontology's class, the step Dai and Zeng treat as its own task [Dai 2023]. Typing by
description alone [Obeidat 2019], by enriched classes [Ouyang 2024] and top-down through the
hierarchy [Komarlu 2024] are the three moves the binding call makes. The candidate-then-judge shape,
with an explicit "none", is the shape of LLM ontology matching [He 2023, Hertling 2023, Babaei
Giglou 2024, Qiang 2024, Song 2025]. Deciding with two votes and the candidate order reversed on the
second comes from the position bias measured in LLM judges and choosers [Zheng 2023, Wang 2024,
Pezeshkpour 2024, Zheng 2024]; agreement across samples as a confidence signal is self-consistency
[Wang 2023a].

**Time resolution is temporal tagging over a bitemporal ledger.** The words of a mention and their
value are TimeML's TIMEX3 [Pustejovsky 2003]; computing the value against a reference time by rule
is what HeidelTime [Strötgen 2012, Strötgen 2013] and SUTime [Chang 2012] do, and TempEval-3 measured
how much harder the value is than the span [UzZaman 2013]. The split we made, a model that returns
an interpretation and code that computes from it, is TimeNorm's grammar given an anchor
[Bethard 2013], SCATE's compositional operators [Bethard 2016a] and, closest of all, the 2025 framing
of normalization as generating a program that a deterministic library executes [Su 2025a]; the
surveys are [Leeuwenberg 2019, Zhong 2023, Su 2025b]. Dating a document is its own task
[Chambers 2012, Vashishth 2018b]; we transcribe the date the document states instead of classifying.
Facts with validity intervals are YAGO2's model, which "never assigns an ill-defined time"
[Hoffart 2013], and the temporal scoping line [Ling 2010, Wang 2011, Talukdar 2012]; the LLM-era
temporal graphs separate observed time from valid time the same way [Rasmussen 2025, Lairgi 2025].
The two axes of [0019] are the valid time and transaction time of bitemporal databases
[Snodgrass 1999, Jensen 1999, Kulkarni 2012], in the words of the consensus glossary [Jensen 1998];
the precision columns are time granularity [Bettini 2000]; `attested_to` is a now-relative value in
the sense of Clifford et al. [Clifford 1997]. The clinical corpora are where relative time was
studied hardest [Sun 2013, Styler 2014].

**A statement with a located quote is KBP provenance and an atomic proposition.** TAC KBP required
a document offset as provenance for every slot fill; FActScore [Min 2023] and Dense X Retrieval
[Chen 2024a] made the atomic proposition the unit of checking and of retrieval, and the quote-then-answer
models [Menick 2022, Huang 2024, Zhang 2025] made the verbatim quote the unit of trust. Our statement
is that unit with its provenance attached at write time [0001, 0044].

## The pitfalls we took

Each item: where it was found, what we do, what is still missing. Items marked *open* are not yet
built.

### Extraction

1. **Uninformative relation phrases.** ReVerb named the two error classes of open extraction,
   incoherent phrases and uninformative ones, its example being "made a deal with" cut to "made"
   [Fader 2011]; the fact-level re-annotation of BenchIE rejected vague relation phrases outright
   [Lamarche 2024]; labelling chunks instead of tokens is one remedy [Dong 2023a]. Our
   "各地区 深入 企业" is ReVerb's second class. We keep the whole sentence as the quote, so nothing
   is lost; the contract states that a phrase is the verb with the words that belong to it, so that
   the statement reads on its own, and the judge reports "reads alone" beside "not stated"
   ([extraction](extraction.md), #743).
2. **A triple the sentence did not assert.** OLLIE showed how many extractions are conditioned,
   attributed or hypothetical in the sentence and false without that context [Mausam 2012];
   speculation is a property of the tuple, not of the sentence [Dong 2023b]. Qualifiers hold the
   context [0037, 0044]; a statement the passage requires, plans, expects or makes conditional
   carries a `mood` qualifier in the passage's own words, a reporting verb is not a mood, and
   alignment never materializes a statement with a mood as a typed fact
   ([extraction](extraction.md), #745). *Open:* the subject of a directive sentence in a notice
   whose addressee is outside the chunk; a rule naming the addressee was tried and withdrawn
   because it emptied a fifth of a filing's chunks.
3. **Minimal is not the same as self-contained.** MinIE says minimization loses context in its own
   evaluation [Gashteovski 2017]; the neural systems went the other way and over-include
   [Fatahi Bayat 2022]; CaRB [Bhardwaj 2019], WiRe57 [Léchelle 2019], LSOIE [Solawetz 2021] and
   BenchIE [Gashteovski 2022] disagree on how compact a fact should be, and rankings flip with the
   matcher. The fact-checking side reached the same place: fully atomic facts are not the right
   unit, decontextuality and minimality pull against each other [Gunjal 2024, Choi 2021], and more
   decomposition is not monotonically better [Hu 2025]. Our rule: one statement is one complete
   proposition; coordinated objects split; a verb chain sharing one object does not (#743).
4. **The decomposition is itself a model output.** Change the decomposition prompt and every
   downstream number moves [Wanner 2024]; a segmentation pre-pass can drop entities for strong
   models [Pommeret 2026]. The contract is versioned in code and each bench round names its
   contract [extraction](extraction.md). *Open:* entity recall measured with and without any
   pre-pass before one is added.
5. **Coordination and verb chains are the yield on dense sentences.** Splitting conjunctive
   sentences first gave up to 1.8× yield with a precision gain [Saha 2018]; a coordination analyser
   inside the extractor is what moved OpenIE6 [Kolluru 2020]; it can be done without a parser
   [Wang 2023b]; split-and-rephrase is the general task [Narayan 2017]; irrelevant context is a
   named failure of LLM OpenIE [Ling 2023]. The judge and the coverage script measure the dense
   NVDA sentences separately (#731). A structural count of the money and percentage figures that
   no statement of their chunk carries (`scripts/bench/figures.mjs`, #748) found 2 of 83 in prose
   on the 25-document batch, 2 of 35 on the NVDA releases and 0 of 451 in their tables, with reasoning
   on at the endpoint; a second-look pass waits for a corpus that loses more.
6. **Common nouns as entities.** On Wikipedia text only 42% of open-extraction arguments are named
   entities [Gashteovski 2019]; canonicalization meets the same string with different referents
   first [Galárraga 2014]; the opposite failure, one referent under many strings, is the sparsity
   KGGen was built against [Mo 2025], and coreference before extraction was the largest lever on
   duplicate nodes in CORE-KG [Meher 2025]. "企业" merged across documents is a hub that means
   nothing. A described thing exists only when a statement points at it [#736]. A thing is named
   only by a proper name or a fixed term that means the same in any document; a role or a generic
   phrase whose referent the passage decides is described, scoped to its document and never merged
   by string; a generic phrase in an adjunct stays words in the qualifier (#743). The gate is a
   term gate, not a proper-name gate: a disease, a drug or an indicator merges across documents by
   design.
7. **A closed schema makes the model invent.** Prompting with a fixed relation set causes
   hallucination [Jiang 2024]; a model forced onto an ontology conforms almost perfectly and is
   still wrong [van Cauter 2024]; the dominant "error" of LLM extraction against gold is spans the
   gold never annotated [Han 2023]; exact match penalises paraphrase, so the honest evaluations are
   human [Wadhwa 2023, Li 2023]. The open contract uses the document's words, the quote must occur
   in the chunk, the run is at temperature 0, and the judge asks whether the text states it rather
   than whether it matches a gold form; "not stated" stays under 2% on every corpus
   ([extraction](extraction.md)). Hallucination and omission are reported as two numbers, as
   Ghanem and Cruz ask [Ghanem 2024]: the judge and the coverage script.
8. **A quote that is a substring is not yet support.** With frontier models 83 to 91% of required
   quotes were even substrings and only 48 to 79% of those supported the claim [Windisch 2026];
   citation presence is not citation support [Gao 2023]; a three-way label (attributable,
   extrapolated, contradictory) is more useful than a binary one [Yue 2023]; abstention must be a
   first-class output [Menick 2022]. `quote_not_in_chunk` is a drop reason and the judge is a
   separate pass; since #749 a second pass marks each "misworded" verdict extrapolated or
   contradicted. The verdict pass itself is untouched, because the judge is sensitive to its own
   wording: the same 909 statements came out 0.8% misworded when the three verdicts were reworded as
   four and 2.1% when the kind was asked in the same call, against 5.0% and 3.6% from two passes of
   the unchanged judge.
9. **The contract's shape has a cost.** Format restrictions degrade reasoning [Tam 2024]; guideline
   prompts are not reliably obeyed without training [Sainz 2024]; quoting before stating improves
   grounding [Huang 2024]; forcing citations finer than a sentence hurts [Wang 2026]. The compact
   contract was chosen by measurement (#731); the quote is the first slot of a statement, so the
   model copies the sentence before it writes from it (#748: on the four NVDA releases 909 statements
   against 901, misworded 5% in both, prose misworded 6% against 9% and prose that does not read
   alone 5% against 11%). *Open:* clause-level quotes measured against minimal ones; a run-to-run
   stability number.
10. **Numbers need their own patterns.** Open numerical extraction is its own problem
    [Saha 2017]; the dense-sentence figures we drop are that class. *Open:* the second-look pass of
    item 5.

### Alignment

11. **Arguments before relations.** Galárraga et al. cluster noun phrases first and relation
    phrases on a semi-canonicalized base; four coarse argument types raised pairwise precision from
    0.946 to 0.997 because polysemous phrases split by signature [Galárraga 2014]; CESI inherits the
    order [Vashishth 2018a]; the mapping of an open relation to a DBpedia property depends on the
    argument types [Dutta 2015]; Angeli's KBP mapping is conditioned on the type signature
    [Angeli 2015]; typed markers lift relation extraction [Ling 2012, Zhong 2021]; ODKE+ exposes only
    the type's slice of the ontology [Khorshidi 2025]; iText2KG lists typing as its missing
    ingredient [Lairgi 2024]. Kind words bind before relation phrases [0044 cut 2]; the phrase binding reads the classes at
    both ends and the (class, class) signature is part of its key (#PRN).
12. **Precision is lost in the binding, not in the extraction.** Of the mapping errors in the
    three-hour KBP system, 31% were definition mismatch and 23% over-generalized rules against 15%
    open-extraction errors [Soderland 2013]; the top hundred predicates cover only 57 to 82% of an
    open base's triples [Romero 2023]; an open statement often maps to a conjunction of ontology
    facts, not one property [Gashteovski 2020]. A word with no fitting class stays untyped and its
    statements stay open; the only output is a suggestion to add a class ([ontology](ontology.md),
    #741). *Open:* a phrase may bind to a formula, not only a property.
13. **Nothing aligns to an undefined class.** EDC's gain comes from definitions written before
    matching and a retriever whose recall bounds what can be bound [Zhang 2024]; definitions help
    both retrieval and the judge [Song 2025]; a class is introduced by its description alone
    [Obeidat 2019] and improved by example names [Ouyang 2024]; written guidelines are what make
    zero-shot extraction work [Sainz 2024]. The signature the aligner reads is the kind word, its
    spellings, example names and the relation phrases its things take part in; candidates come
    from the ontology's embedding index when the base has one, else the whole list when it is small
    (#741). *Open:* a class without a definition is not a binding target, and the ontology page
    says so.
14. **The characteristic error is the wrong level, not the wrong branch.** Naive LLM matching binds
    a subclass to its parent [Norouzi 2023]; across ten models the errors are align-up, align-down
    and disputed, with few outright false mappings [Qiang 2025]; order sensitivity concentrates
    where the model is torn between its top options [Pezeshkpour 2024]; resolving top-down through
    the hierarchy helps [Komarlu 2024]. *Open:* the queue card for an undecided word shows each
    candidate with its parent and children; a candidate list includes the parent of every
    candidate.
15. **The judge prefers the first option.** Swapping two answers changed GPT-4's verdict in a third
    of cases and Claude-v1's in three quarters [Zheng 2023]; reordering alone reversed a
    leaderboard [Wang 2024]; selection bias comes from the prior on option-ID tokens [Zheng 2024].
    Two votes, the second with candidates reversed; candidates are named by class key, never by a
    letter; disagreement is a queue item, not a binding (#741). Spending the model's votes only on
    borderline cases is the next economy [Taboada 2025].
16. **Conformance is not correctness.** Ontology conformance near 1.0 with hallucinated arguments
    [van Cauter 2024]; the standard metric names are conformance and hallucination rate
    [Mihindukulasooriya 2023]; the standard check for the no-match case is a rejection rate over
    concepts that have no counterpart [He 2023], now part of the OAEI Bio-ML evaluation [OAEI 2024].
    *Open:* the alignment bench seeds kind words that fit no class and counts how often the aligner
    binds them anyway.
17. **Candidate recall is the ceiling.** Whatever the retriever misses can never be bound [Hertling
    2023, Zhang 2024]; the similarity threshold is a precision dial tuned per ontology [Qiang 2024];
    the number of clusters is estimated, not set [Shen 2022]. The whole list goes to the model when
    the base has at most sixty classes. *Open:* candidate recall measured on the batch base before
    the embedding index is trusted above sixty.
18. **A role is not a kind.** A class that names the role a thing plays in a relation cannot be
    assigned from the thing's own kind word; Soderland et al. hit it with BombingAgent
    [Soderland 2010]. *Open:* classes declared as roles are excluded from kind-word candidates.
19. **A pipeline that cannot be re-run drifts.** Mappings are partial and revisable by design
    [Madhavan 2007]; error propagation from extraction into fusion is the survey's standing warning
    [Bian 2025]; the surveys ask for provenance and re-runnable stages [Weikum 2021, Hogan 2021].
    Every binding carries its two votes and a `decided_at` compared with the class's `updated_at`;
    only stale bindings are re-decided; every statement keeps its quote and offsets.

### Time

20. **The document type chooses the anchor, and the anchor is where the value goes wrong.** With
    the same extractor, anchoring narrative text to the document's date instead of the previous
    mention drops the value score from 89.8 to 64.9, and anchoring clinical-trial text to a local
    time point zero raises it from 74.4 to 88.7 [Strötgen 2012]; in clinical narratives the anchor
    choice is the least accurate component, at 74.7% [Sun 2015], and anchor errors are the top
    cause of wrong values [Olex 2022]; a discharge summary has two legitimate document times,
    admission and discharge [Sun 2013]. The document context is read once, mention anchors are
    resolved in a second pass, and the anchor is stored in the interpretation, so a value always
    says what it was computed from [0045]. *Open:* the strategy is chosen per document type; a
    trial-relative mention ("week 4") is kept as an offset from a named local anchor in the manner
    of HeidelTime's time point zero and THYME's event-anchored expressions [Styler 2014], so that it
    becomes grade B the day the anchor is dated, instead of grade C forever.
21. **Normalizing is harder than finding.** In TempEval-3 the best system found mentions at 90.3
    but gave the right value at 77.6, and every participant normalized by rules [UzZaman 2013];
    the neural parsers score operators such as "last" and "next" at a third of the rest
    [Laparra 2018]; relative and underspecified mentions stay the hardest for prompted models
    [Gautam 2024]. The model gives only an interpretation; code computes the value; nothing is
    written when it cannot [0045]. The strongest recent version of that split generates a program
    and executes it [Su 2025a]; the vocabulary of the code side is the ten operations of ARTime
    [Ding 2021]. *Open:* the time bench scores a resolved interval by overlap with the reference,
    not by string equality [Laparra 2018]; an ambiguous mention may return alternatives rather than
    one value [Bethard 2013].
22. **A wrong document date poisons every relative expression.** No study manipulates the document
    date alone; the anchor numbers of item 20 are the quantification. The document's own stated
    dates are the best evidence of its date [Chambers 2012]; year-level dating is useless for a
    quarter [Vashishth 2018b] and dating from implicit cues is decades coarse [Kumar 2012]. The
    agent-memory graphs of 2025 anchor relative dates to the message timestamp and collapse a date
    to midnight [Rasmussen 2025], the two errors this ledger forbids; ATOM separates observed time
    from valid time and measures run-to-run stability [Lairgi 2025]. Upload time and file
    modification time are never a document date [0045, #714]; anything computed from the document
    date is grade B. *Open:* a person changing the document date triggers re-resolution
    (0045 cut 4); run-to-run stability of the interpretation call is a bench number.
23. **A fiscal period is its own granularity.** Fiscal quarters and 52/53-week years are
    non-regular granularities, and truncation to a calendar month is only defined when the target
    groups the source [Bettini 2000]; a 52/53-week quarter truncated to the month is off by up to
    six days. SEC filings carry the fiscal year end and a period code (FY, Q1 to Q4, H1, H2, M9, T1
    to T3) in their XBRL metadata [SEC FSDS]. Fiscal quarters are computed from the fiscal year end
    the opening states [0045]. *Open:* half-years and nine-month periods are period codes the
    resolver accepts; the fiscal year end is read from the filing's metadata when the source is
    EDGAR; fiscal periods are one granularity system beside the calendar, not a calendar month.
24. **Mixing the two axes corrupts history.** Snodgrass's book is one long warning
    [Snodgrass 1999]; transaction time is system-assigned and append-only, valid time is asserted
    [Jensen 1999]; record time is the commit instant, assigned once and never back-dated
    [Torp 2000]; SQL:2011 periods have neither a granularity nor an unknown end [Kulkarni 2012].
    Two axes with separate predicates [0019]; the precision columns and the ended-unknown shape are
    stated as extensions beyond the standard. *Open:* [time](time.md) names the axes once in the
    glossary's words, valid and transaction [Jensen 1998].
25. **Unknown is three different things.** Wikidata distinguishes a value, an unknown value that
    exists, and no value; an end time is "unknown value" when the thing ended on an unknown date
    and absent when it is ongoing [Wikidata Help]; the TimeBank anchoring corpus annotates a point,
    an interval, a lower bound, an upper bound or nothing [Reimers 2016]; YAGO2 leaves a fact
    without a time rather than give it an ill-defined one [Hoffart 2013]. Ours are `until`,
    `ended_unknown` with `attested_to`, and an open end with `attested_to` [0022, 0045], and grade C
    writes nothing. `attested_to` means true as of the last attestation, a now-relative value that
    is stored as an explicit timestamp and evaluated when asked [Clifford 1997]; a fact whose last
    attestation is old is stale, not false [Soulard 2025].
26. **A table is a structure, not a paragraph.** The financial-report question-answering sets
    feed a table to the model one row at a time with its row and column headers written out
    [Chen 2021, Zhu 2021], and the pretrained table models flatten it without losing which row and
    column a cell sits in [Herzig 2020]; the standard for turning cells into one faithful sentence
    is ToTTo [Parikh 2020]; the statistical-report tables with hierarchical headers have their own
    dataset, whose difficulty is "complex hierarchical indexing" [Cheng 2022, Zhao 2022]; how a
    table is serialised, ordered and partitioned changes what a model reads from it [Sui 2024], a
    change of structure with the same content costs accuracy until the structure is normalised
    [Liu 2023], a whole table is not fed to a model at all when it is large [Chen 2024c], and the
    survey of the field is [Fang 2024]; which column is the subject and which are properties is
    the semantic table interpretation task [Cafarella 2008, SemTab]. Under #743 the earnings release lost its column headings between the HTML converter
    and the chunker and the model wrote the period as the phrase. Tables are now rendered from the
    DOM by structure, one row per line under its real headings, the section path folded into the
    label, the caption run kept with every chunk ([sources](sources.md), #744); a heading that
    names a time is `when`, a heading that names a change between periods stays the phrase; a
    phrase equal to the value or to the subject is kept and counted. DOCX tables, spreadsheets and
    CSV files are laid on the same grid by their parsers (#750): a DOCX cell keeps its span and
    the indent of its first paragraph, a sheet's or a file's first row of two or more cells is its
    header even when the headings are years, and a table with no figure in it is a header row and
    records. *Open:* the text layer of a PDF carries no table structure (a scanned PDF's tables
    arrive from MinerU as pipe tables already); the paragraph units of
    the NVDA cross-domain corpus are recut from rendered rows with their headings, and the reference
    facts carry the period; a check that a cell statement under a period column carries `when`.
27. **Precision is not confidence, and vague is not coarse.** An indeterminate instant is a set of
    chronons with a distribution, and comparing two of them needs a stated semantics
    [Dyreson 1998]; confidence and bitemporality are separate axes [Chekol 2018]; "early 2019" is a
    modifier with a calibrated width, not a coarser precision [Tissot 2019]; grading a date by the
    provenance of its anchor is uncertainty typing, which the survey prefers to a scalar
    [Jarnac 2025]. A precision column on every bound, with CHECK constraints that truncate the
    value [0024]; the grade is a letter beside the value, never a probability. *Open:* the
    cross-precision comparison used by the temporal engine, overlap or containment, is written
    down in [time](time.md); a vague modifier is a qualifier on the mention, not a precision.

## What has no precedent

These decisions have no paper behind them and are held up only by our own benches
([extraction](extraction.md), the batch review of #731, #740 and #741):

- A described thing becomes an entity only when a statement points at it [#736].
- The contract names things by their words, not by ids; ids mixed on dense passages [#731].
- Temperature 0 and a verbatim quote as the condition of entry; "not stated" ≤ 2% on every corpus.
- The three grades of a resolved time as a letter beside the value [0045]. YAGO2's refusal to write
  an ill-defined time and the survey's preference for typed uncertainty are precedents in spirit,
  not in form.
- Every intermediate layer stays in the ledger and is re-run by a job, instead of a pipeline that
  computes once.

Three things the survey looked for and did not find: a controlled comparison of schema-first and
schema-late extraction with the same extractor; a paper that studies generic-noun hubs as a named
phenomenon; a study that manipulates the document date alone and measures what it does to relative
mentions. All three are ours to measure.

## References

Every entry was verified against the linked page. Grouped by the layer that cites it; a paper cited
by two layers is listed once.

### Open extraction

- [Banko 2007] Banko, Cafarella, Soderland, Broadhead, Etzioni. Open Information Extraction from the Web. IJCAI 2007. https://www.ijcai.org/Proceedings/07/Papers/429.pdf
- [Fader 2011] Fader, Soderland, Etzioni. Identifying Relations for Open Information Extraction. EMNLP 2011. https://aclanthology.org/D11-1142/
- [Mausam 2012] Mausam, Schmitz, Bart, Soderland, Etzioni. Open Language Learning for Information Extraction. EMNLP-CoNLL 2012. https://aclanthology.org/D12-1048/
- [Del Corro 2013] Del Corro, Gemulla. ClausIE: Clause-Based Open Information Extraction. WWW 2013. https://dl.acm.org/doi/10.1145/2488388.2488420
- [Angeli 2015] Angeli, Johnson Premkumar, Manning. Leveraging Linguistic Structure For Open Domain Information Extraction. ACL 2015. https://aclanthology.org/P15-1034/
- [Stanovsky 2018] Stanovsky, Michael, Zettlemoyer, Dagan. Supervised Open Information Extraction. NAACL 2018. https://aclanthology.org/N18-1081/
- [Kolluru 2020] Kolluru, Adlakha, Aggarwal, Mausam, Chakrabarti. OpenIE6: Iterative Grid Labeling and Coordination Analysis for Open Information Extraction. EMNLP 2020. https://aclanthology.org/2020.emnlp-main.306/
- [Niklaus 2018] Niklaus, Cetto, Freitas, Handschuh. A Survey on Open Information Extraction. COLING 2018. https://aclanthology.org/C18-1326/
- [Zhou 2022] Zhou et al. A Survey on Neural Open Information Extraction: Current Status and Future Directions. IJCAI 2022. https://www.ijcai.org/proceedings/2022/793
- [Liu 2024] Liu et al. A Survey on Open Information Extraction from Rule-based Model to Large Language Model. Findings of EMNLP 2024. https://aclanthology.org/2024.findings-emnlp.560/
- [Pei 2023] Pei et al. Abstractive Open Information Extraction. EMNLP 2023. https://aclanthology.org/2023.emnlp-main.376/
- [Tseng 2014] Tseng, Lee, Lin, Liao, Liu, Chen, Etzioni, Fader. Chinese Open Relation Extraction for Knowledge Acquisition. EACL 2014. https://aclanthology.org/E14-4003/
- [Qiu 2014] Qiu, Zhang. ZORE: A Syntax-based System for Chinese Open Relation Extraction. EMNLP 2014. https://aclanthology.org/D14-1201/
- [Sun 2018] Sun, Li, Wang, Fan, Feng, Li. Logician: A Unified End-to-End Neural Approach for Open-Domain Information Extraction. WSDM 2018. https://arxiv.org/abs/1904.12535
- [Deng 2022] Deng et al. Title2Event: Benchmarking Open Event Extraction with a Large-scale Chinese Title Dataset. EMNLP 2022. https://aclanthology.org/2022.emnlp-main.437/
- [Abolhasani 2026] Abolhasani, Ba, He, Pan. Beyond Predefined Schemas: TRACE-KG for Context-Enriched Knowledge Graph Generation. 2026. https://arxiv.org/abs/2604.03496
- [Han 2026] Han, Lai. From Symbolic to Natural-Language Relations: Rethinking Knowledge Graph Construction in the Era of Large Language Models. 2026. https://arxiv.org/abs/2601.09069
- [Dong 2023a] Dong et al. Open Information Extraction via Chunks. EMNLP 2023. https://aclanthology.org/2023.emnlp-main.951/
- [Dong 2023b] Dong et al. From Speculation Detection to Trustworthy Relational Tuples in Information Extraction. Findings of EMNLP 2023. https://aclanthology.org/2023.findings-emnlp.886/
- [Ling 2023] Ling et al. Improving Open Information Extraction with Large Language Models: A Study on Demonstration Uncertainty. 2023. https://arxiv.org/abs/2309.03433
- [Saha 2017] Saha, Pal, Mausam. Bootstrapping for Numerical Open IE. ACL 2017. https://aclanthology.org/P17-2050/
- [Saha 2018] Saha, Mausam. Open Information Extraction from Conjunctive Sentences. COLING 2018. https://aclanthology.org/C18-1194/
- [Wang 2023b] Wang et al. CoRec: An Easy Approach for Coordination Recognition. EMNLP 2023. https://aclanthology.org/2023.emnlp-main.934/
- [Narayan 2017] Narayan, Gardent, Cohen, Shimorina. Split and Rephrase. EMNLP 2017. https://aclanthology.org/D17-1064/

### Granularity, qualifiers, propositions

- [Gashteovski 2017] Gashteovski, Gemulla, Del Corro. MinIE: Minimizing Facts in Open Information Extraction. EMNLP 2017. https://aclanthology.org/D17-1278/
- [Bhutani 2016] Bhutani, Jagadish, Radev. Nested Propositions in Open Information Extraction. EMNLP 2016. https://aclanthology.org/D16-1006/
- [Niklaus 2019] Niklaus, Cetto, Freitas, Handschuh. Transforming Complex Sentences into a Semantic Hierarchy. ACL 2019. https://aclanthology.org/P19-1333/
- [Vrandečić 2014] Vrandečić, Krötzsch. Wikidata: A Free Collaborative Knowledgebase. Communications of the ACM 57(10), 2014. https://dl.acm.org/doi/10.1145/2629489
- [Chia 2022] Chia et al. A Dataset for Hyper-Relational Extraction and a Cube-Filling Approach. EMNLP 2022. https://aclanthology.org/2022.emnlp-main.688/
- [Luo 2024] Luo et al. Text2NKG: Fine-Grained N-ary Relation Extraction for N-ary relational Knowledge Graph Construction. NeurIPS 2024. https://arxiv.org/abs/2310.05185
- [Fatahi Bayat 2022] Fatahi Bayat, Bhutani, Jagadish. CompactIE: Compact Facts in Open Information Extraction. NAACL 2022. https://aclanthology.org/2022.naacl-main.65/
- [Bhardwaj 2019] Bhardwaj, Aggarwal, Mausam. CaRB: A Crowdsourced Benchmark for Open IE. EMNLP-IJCNLP 2019. https://aclanthology.org/D19-1651/
- [Léchelle 2019] Léchelle, Gotti, Langlais. WiRe57: A Fine-Grained Benchmark for Open Information Extraction. LAW 2019. https://aclanthology.org/W19-4002/
- [Solawetz 2021] Solawetz, Larson. LSOIE: A Large-Scale Dataset for Supervised Open Information Extraction. EACL 2021. https://aclanthology.org/2021.eacl-main.222/
- [Gashteovski 2022] Gashteovski, Yu, Kotnis, Lawrence, Niepert, Glavaš. BenchIE: A Framework for Multi-Faceted Fact-Based Open Information Extraction Evaluation. ACL 2022. https://aclanthology.org/2022.acl-long.307/
- [Lamarche 2024] Lamarche, Langlais. BenchIE^FL: A Manually Re-Annotated Fact-Based Open Information Extraction Benchmark. Findings of ACL 2024. https://aclanthology.org/2024.findings-acl.496/
- [Gunjal 2024] Gunjal, Durrett. Molecular Facts: Desiderata for Decontextualization in LLM Fact Verification. Findings of EMNLP 2024. https://aclanthology.org/2024.findings-emnlp.215/
- [Choi 2021] Choi, Palomaki, Lamm, Kwiatkowski, Das, Collins. Decontextualization: Making Sentences Stand-Alone. TACL 2021. https://aclanthology.org/2021.tacl-1.27/
- [Hu 2025] Hu et al. Decomposition Dilemmas: Does Claim Decomposition Boost or Burden Fact-Checking Performance? NAACL 2025. https://aclanthology.org/2025.naacl-long.320/
- [Wanner 2024] Wanner et al. A Closer Look at Claim Decomposition. *SEM 2024. https://aclanthology.org/2024.starsem-1.13/
- [Pommeret 2026] Pommeret et al. LLM-based Atomic Propositions help weak extractors: Evaluation of a Propositioner for triplet extraction. 2026. https://arxiv.org/abs/2604.02866
- [Min 2023] Min et al. FActScore: Fine-grained Atomic Evaluation of Factual Precision in Long Form Text Generation. EMNLP 2023. https://aclanthology.org/2023.emnlp-main.741/
- [Chen 2024a] Chen et al. Dense X Retrieval: What Retrieval Granularity Should We Use? EMNLP 2024. https://aclanthology.org/2024.emnlp-main.845/

### Grounding and evaluation of model extraction

- [Jiang 2024] Jiang et al. GenRES: Rethinking Evaluation for Generative Relation Extraction in the Era of Large Language Models. NAACL 2024. https://aclanthology.org/2024.naacl-long.155/
- [Han 2023] Han et al. Is Information Extraction Solved by ChatGPT? An Analysis of Performance, Evaluation Criteria, Robustness and Errors. 2023. https://arxiv.org/abs/2305.14450
- [Wadhwa 2023] Wadhwa, Amir, Wallace. Revisiting Relation Extraction in the era of Large Language Models. ACL 2023. https://aclanthology.org/2023.acl-long.868/
- [Li 2023] Li et al. Evaluating ChatGPT's Information Extraction Capabilities: An Assessment of Performance, Explainability, Calibration, and Faithfulness. 2023. https://arxiv.org/abs/2304.11633
- [Ghanem 2024] Ghanem, Cruz. Enhancing Knowledge Graph Construction: Evaluating with Emphasis on Hallucination, Omission, and Graph Similarity Metrics. KGSWC 2024. https://arxiv.org/abs/2502.05239
- [Windisch 2026] Windisch et al. Show Your Work: Verbatim Evidence Requirements and Automated Assessment of Large Language Models for Biomedical Text Processing of Trial Eligibility Criteria. Cureus 2026. https://pmc.ncbi.nlm.nih.gov/articles/PMC13252682/
- [Gao 2023] Gao, Yen, Yu, Chen. Enabling Large Language Models to Generate Text with Citations. EMNLP 2023. https://aclanthology.org/2023.emnlp-main.398/
- [Yue 2023] Yue et al. Automatic Evaluation of Attribution by Large Language Models. Findings of EMNLP 2023. https://aclanthology.org/2023.findings-emnlp.307/
- [Menick 2022] Menick et al. Teaching language models to support answers with verified quotes. 2022. https://arxiv.org/abs/2203.11147
- [Huang 2024] Huang et al. Learning Fine-Grained Grounded Citations for Attributed Large Language Models. Findings of ACL 2024. https://aclanthology.org/2024.findings-acl.838/
- [Zhang 2025] Zhang et al. Verifiable by Design: Aligning Language Models to Quote from Pre-Training Data. NAACL 2025. https://aclanthology.org/2025.naacl-long.191/
- [Tam 2024] Tam et al. Let Me Speak Freely? A Study on the Impact of Format Restrictions on Large Language Model Performance. EMNLP 2024 Industry Track. https://aclanthology.org/2024.emnlp-industry.91/
- [Sainz 2024] Sainz et al. GoLLIE: Annotation Guidelines improve Zero-Shot Information-Extraction. ICLR 2024. https://arxiv.org/abs/2310.03668
- [Wang 2026] Wang et al. Are Finer Citations Always Better? Rethinking Granularity for Attributed Generation. 2026. https://arxiv.org/abs/2604.01432

### Canonicalization and alignment

- [Galárraga 2014] Galárraga, Heitz, Murphy, Suchanek. Canonicalizing Open Knowledge Bases. CIKM 2014. https://dl.acm.org/doi/10.1145/2661829.2662073
- [Vashishth 2018a] Vashishth, Jain, Talukdar. CESI: Canonicalizing Open Knowledge Bases using Embeddings and Side Information. WWW 2018. https://dl.acm.org/doi/10.1145/3178876.3186030
- [Dash 2021] Dash, Rossiello, Mihindukulasooriya, Bagchi, Gliozzo. Open Knowledge Graphs Canonicalization using Variational Autoencoders. EMNLP 2021. https://aclanthology.org/2021.emnlp-main.811/
- [Liu 2021] Liu et al. Joint Open Knowledge Base Canonicalization and Linking. SIGMOD 2021. https://dl.acm.org/doi/10.1145/3448016.3452776
- [Shen 2022] Shen, Yang, Liu. Multi-View Clustering for Open Knowledge Base Canonicalization. KDD 2022. https://dl.acm.org/doi/10.1145/3534678.3539449
- [Yang 2024] Yang, Curry. Open Knowledge Base Canonicalization: Techniques and Challenges. TEXT2KG 2024 (ESWC), CEUR Vol-3747. https://ceur-ws.org/Vol-3747/text2kg_paper5.pdf
- [Gashteovski 2019] Gashteovski, Wanner, Hertling, Broscheit, Gemulla. OPIEC: An Open Information Extraction Corpus. AKBC 2019. https://arxiv.org/abs/1904.12324
- [Gashteovski 2020] Gashteovski et al. On Aligning OpenIE Extractions with Knowledge Bases: A Case Study. Eval4NLP 2020. https://aclanthology.org/2020.eval4nlp-1.14/
- [Soderland 2010] Soderland, Roof, Qin, Xu, Mausam, Etzioni. Adapting Open Information Extraction to Domain-Specific Relations. AI Magazine 31(3), 2010. https://ojs.aaai.org/index.php/aimagazine/article/view/2305
- [Soderland 2013] Soderland, Gilmer, Bart, Etzioni, Weld. Open Information Extraction to KBP Relations in 3 Hours. TAC 2013. https://tac.nist.gov/publications/2013/participant.papers/UWashington.TAC2013.proceedings.pdf
- [Dutta 2015] Dutta, Meilicke, Stuckenschmidt. Enriching Structured Knowledge with Open Information. WWW 2015. https://dl.acm.org/doi/10.1145/2736277.2741139
- [Romero 2023] Romero, Razniewski. Mapping and Cleaning Open Commonsense Knowledge Bases with Generative Translation. ISWC 2023. https://arxiv.org/abs/2306.12766
- [Zhang 2024] Zhang, Soh. Extract, Define, Canonicalize: An LLM-based Framework for Knowledge Graph Construction. EMNLP 2024. https://aclanthology.org/2024.emnlp-main.548/
- [Mo 2025] Mo et al. KGGen: Extracting Knowledge Graphs from Plain Text with Language Models. NeurIPS 2025. https://arxiv.org/abs/2502.09956
- [Lairgi 2024] Lairgi, Moncla, Cazabet, Benabdeslem, Cléau. iText2KG: Incremental Knowledge Graphs Construction Using Large Language Models. WISE 2024. https://arxiv.org/abs/2409.03284
- [Meher 2025] Meher, Domeniconi. Inside CORE-KG: Evaluating Structured Prompting and Coreference Resolution for Knowledge Graphs. 2025. https://arxiv.org/abs/2510.26512
- [Bai 2025] Bai et al. AutoSchemaKG: Autonomous Knowledge Graph Construction through Dynamic Schema Induction from Web-Scale Corpora. 2025. https://arxiv.org/abs/2505.23628
- [Chen 2024b] Chen et al. SAC-KG: Exploiting Large Language Models as Skilled Automatic Constructors for Domain Knowledge Graph. ACL 2024. https://aclanthology.org/2024.acl-long.238/
- [Sun 2025] Sun et al. Docs2KG: A Human-LLM Collaborative Approach to Unified Knowledge Graph Construction from Heterogeneous Documents. WWW Companion 2025. https://arxiv.org/abs/2406.02962
- [Khorshidi 2025] Khorshidi et al. ODKE+: Ontology-Guided Open-Domain Knowledge Extraction with LLMs. 2025. https://arxiv.org/abs/2509.04696
- [Bian 2025] Bian. LLM-empowered knowledge graph construction: A survey. 2025. https://arxiv.org/abs/2510.20345
- [Mihindukulasooriya 2023] Mihindukulasooriya, Tiwari, Enguix, Lata. Text2KGBench: A Benchmark for Ontology-Driven Knowledge Graph Generation from Text. ISWC 2023. https://arxiv.org/abs/2308.02357
- [van Cauter 2024] van Cauter, Yakovets. Ontology-guided Knowledge Graph Construction from Maintenance Short Texts. KaLLM 2024 (ACL). https://aclanthology.org/2024.kallm-1.8/

### Typing

- [Choi 2018] Choi, Levy, Choi, Zettlemoyer. Ultra-Fine Entity Typing. ACL 2018. https://aclanthology.org/P18-1009/
- [Dai 2023] Dai, Zeng. From Ultra-Fine to Fine: Fine-tuning Ultra-Fine Entity Typing Models to Fine-grained. ACL 2023. https://aclanthology.org/2023.acl-long.126/
- [Obeidat 2019] Obeidat, Fern, Shahbazi, Tadepalli. Description-Based Zero-shot Fine-Grained Entity Typing. NAACL 2019. https://aclanthology.org/N19-1087/
- [Ouyang 2024] Ouyang, Huang, Pillai, Zhang, Zhang, Han. Ontology Enrichment for Effective Fine-grained Entity Typing. KDD 2024. https://dl.acm.org/doi/10.1145/3637528.3671857
- [Komarlu 2024] Komarlu, Jiang, Wang, Han. OntoType: Ontology-Guided and Pre-Trained Language Model Assisted Fine-Grained Entity Typing. KDD 2024. https://dl.acm.org/doi/10.1145/3637528.3671745
- [Ling 2012] Ling, Weld. Fine-Grained Entity Recognition. AAAI 2012. https://ojs.aaai.org/index.php/AAAI/article/view/8122
- [Zhong 2021] Zhong, Chen. A Frustratingly Easy Approach for Entity and Relation Extraction. NAACL 2021. https://aclanthology.org/2021.naacl-main.5/

### Matching and judging

- [He 2023] He, Chen, Dong, Horrocks. Exploring Large Language Models for Ontology Alignment. ISWC 2023 Posters & Demos. https://arxiv.org/abs/2309.07172
- [Hertling 2023] Hertling, Paulheim. OLaLa: Ontology Matching with Large Language Models. K-CAP 2023. https://dl.acm.org/doi/10.1145/3587259.3627571
- [Babaei Giglou 2024] Babaei Giglou, D'Souza, Engel, Auer. LLMs4OM: Matching Ontologies with Large Language Models. 2024. https://arxiv.org/abs/2404.10317
- [Qiang 2024] Qiang, Wang, Taylor. Agent-OM: Leveraging LLM Agents for Ontology Matching. PVLDB 18(3), 2024. https://www.vldb.org/pvldb/vol18/p516-qiang.pdf
- [Norouzi 2023] Norouzi, Mahdavinejad, Hitzler. Conversational Ontology Alignment with ChatGPT. 2023. https://arxiv.org/abs/2308.09217
- [Qiang 2025] Qiang, Taylor, Wang, Jiang. OAEI-LLM-T: A TBox Benchmark Dataset for Understanding Large Language Model Hallucinations in Ontology Matching. 2025. https://arxiv.org/abs/2503.21813
- [OAEI 2024] Abd Nikooie Pour et al. Results of the Ontology Alignment Evaluation Initiative 2024. OM 2024 (ISWC), CEUR Vol-3897. https://ceur-ws.org/Vol-3897/oaei2024_paper0.pdf
- [Taboada 2025] Taboada, Martinez, Arideh, Mosquera. Ontology Matching with Large Language Models and Prioritized Depth-First Search. 2025. https://arxiv.org/abs/2501.11441
- [Song 2025] Song, Chen, Schmidt. GenOM: Ontology Matching with Description Generation and Large Language Model. OM 2025 (ISWC), CEUR Vol-4144. https://ceur-ws.org/Vol-4144/om2025-LTpaper3.pdf
- [Zheng 2023] Zheng et al. Judging LLM-as-a-Judge with MT-Bench and Chatbot Arena. NeurIPS 2023 Datasets and Benchmarks. https://arxiv.org/abs/2306.05685
- [Wang 2024] Wang et al. Large Language Models are not Fair Evaluators. ACL 2024. https://aclanthology.org/2024.acl-long.511/
- [Pezeshkpour 2024] Pezeshkpour, Hruschka. Large Language Models Sensitivity to The Order of Options in Multiple-Choice Questions. Findings of NAACL 2024. https://aclanthology.org/2024.findings-naacl.130/
- [Zheng 2024] Zheng, Zhou, Meng, Zhou, Huang. Large Language Models Are Not Robust Multiple Choice Selectors. ICLR 2024. https://arxiv.org/abs/2309.03882
- [Wang 2023a] Wang et al. Self-Consistency Improves Chain of Thought Reasoning in Language Models. ICLR 2023. https://arxiv.org/abs/2203.11171

### Integration and surveys

- [Franklin 2005] Franklin, Halevy, Maier. From Databases to Dataspaces: A New Abstraction for Information Management. SIGMOD Record 34(4), 2005. https://dl.acm.org/doi/10.1145/1107499.1107502
- [Halevy 2006] Halevy, Franklin, Maier. Principles of Dataspace Systems. PODS 2006. https://dl.acm.org/doi/10.1145/1142351.1142352
- [Madhavan 2007] Madhavan et al. Web-scale Data Integration: You can only afford to Pay As You Go. CIDR 2007. https://www.cidrdb.org/cidr2007/papers/cidr07p40.pdf
- [Poggi 2008] Poggi, Lembo, Calvanese, De Giacomo, Lenzerini, Rosati. Linking Data to Ontologies. Journal on Data Semantics X, LNCS 4900, 2008. https://link.springer.com/chapter/10.1007/978-3-540-77688-8_5
- [Xiao 2018] Xiao et al. Ontology-Based Data Access: A Survey. IJCAI 2018. https://www.ijcai.org/proceedings/2018/777
- [Hogan 2021] Hogan et al. Knowledge Graphs. ACM Computing Surveys 54(4), 2021. https://arxiv.org/abs/2003.02320
- [Weikum 2021] Weikum, Dong, Razniewski, Suchanek. Machine Knowledge: Creation and Curation of Comprehensive Knowledge Bases. Foundations and Trends in Databases 10(2–4), 2021. https://arxiv.org/abs/2009.11564

### Tables

- [Chen 2021] Chen et al. FinQA: A Dataset of Numerical Reasoning over Financial Data. EMNLP 2021. https://aclanthology.org/2021.emnlp-main.300/
- [Zhu 2021] Zhu et al. TAT-QA: A Question Answering Benchmark on a Hybrid of Tabular and Textual Content in Finance. ACL-IJCNLP 2021. https://aclanthology.org/2021.acl-long.254/
- [Parikh 2020] Parikh et al. ToTTo: A Controlled Table-To-Text Generation Dataset. EMNLP 2020. https://aclanthology.org/2020.emnlp-main.89/
- [Herzig 2020] Herzig et al. TaPas: Weakly Supervised Table Parsing via Pre-training. ACL 2020. https://aclanthology.org/2020.acl-main.398/
- [Cheng 2022] Cheng et al. HiTab: A Hierarchical Table Dataset for Question Answering and Natural Language Generation. ACL 2022. https://aclanthology.org/2022.acl-long.78/
- [Zhao 2022] Zhao et al. MultiHiertt: Numerical Reasoning over Multi Hierarchical Tabular and Textual Data. ACL 2022. https://aclanthology.org/2022.acl-long.454/
- [Cafarella 2008] Cafarella, Halevy, Wang, Wu, Zhang. WebTables: Exploring the Power of Tables on the Web. VLDB 2008. https://www.vldb.org/pvldb/vol1/1453916.pdf
- [SemTab] Semantic Web Challenge on Tabular Data to Knowledge Graph Matching, ISWC, since 2019. https://www.cs.ox.ac.uk/isg/challenges/sem-tab/
- [Sui 2024] Sui et al. Table Meets LLM: Can Large Language Models Understand Structured Table Data? A Benchmark and Empirical Study. WSDM 2024. https://arxiv.org/abs/2305.13062
- [Liu 2023] Liu et al. Rethinking Tabular Data Understanding with Large Language Models. 2023. https://arxiv.org/abs/2312.16702
- [Chen 2024c] Chen et al. TableRAG: Million-Token Table Understanding with Language Models. NeurIPS 2024. https://arxiv.org/abs/2410.04739
- [Fang 2024] Fang et al. Large Language Models (LLMs) on Tabular Data: Prediction, Generation, and Understanding. A Survey. TMLR 2024. https://arxiv.org/abs/2402.17944

### Time: tagging and normalization

- [Pustejovsky 2003] Pustejovsky et al. TimeML: Robust Specification of Event and Temporal Expressions in Text. New Directions in Question Answering (AAAI Spring Symposium), 2003. https://aaai.org/papers/0005-ss03-07-005-timeml-robust-specification-of-event-and-temporal-expressions-in-text/
- [UzZaman 2013] UzZaman, Llorens, Derczynski, Allen, Verhagen, Pustejovsky. SemEval-2013 Task 1: TempEval-3: Evaluating Time Expressions, Events, and Temporal Relations. SemEval 2013. https://aclanthology.org/S13-2001/
- [Chang 2012] Chang, Manning. SUTime: A library for recognizing and normalizing time expressions. LREC 2012. https://aclanthology.org/L12-1122/
- [Strötgen 2012] Strötgen, Gertz. Temporal Tagging on Different Domains: Challenges, Strategies, and Gold Standards. LREC 2012. https://aclanthology.org/L12-1219/
- [Strötgen 2013] Strötgen, Gertz. Multilingual and cross-domain temporal tagging. Language Resources and Evaluation 47(2), 2013. https://link.springer.com/article/10.1007/s10579-012-9179-y
- [Bethard 2013] Bethard. A Synchronous Context Free Grammar for Time Normalization. EMNLP 2013. https://aclanthology.org/D13-1078/
- [Bethard 2016a] Bethard, Parker. A Semantically Compositional Annotation Scheme for Time Normalization. LREC 2016. https://aclanthology.org/L16-1599/
- [Laparra 2018] Laparra, Xu, Bethard. From Characters to Time Intervals: New Paradigms for Evaluation and Neural Parsing of Time Normalizations. TACL 6, 2018. https://aclanthology.org/Q18-1025/
- [Ding 2021] Ding, Chen, Li, Qu. Automatic Rule Generation for Time Expression Normalization. Findings of EMNLP 2021. https://aclanthology.org/2021.findings-emnlp.269/
- [Gautam 2024] Gautam, Lange, Strötgen. Discourse-Aware In-Context Learning for Temporal Expression Normalization. NAACL 2024. https://aclanthology.org/2024.naacl-short.27/
- [Su 2025a] Su, Yu, Howard, Bethard. A Semantic Parsing Framework for End-to-End Time Normalization. NeurIPS 2025. https://arxiv.org/abs/2507.06450
- [Su 2025b] Su, Howard, Bethard. Transformer-Based Temporal Information Extraction and Application: A Review. EMNLP 2025. https://aclanthology.org/2025.emnlp-main.1467/
- [Zhong 2023] Zhong, Cambria. Time expression recognition and normalization: a survey. Artificial Intelligence Review 56(9), 2023. https://link.springer.com/article/10.1007/s10462-023-10400-y
- [Leeuwenberg 2019] Leeuwenberg, Moens. A Survey on Temporal Reasoning for Temporal Information Extraction from Text. JAIR, 2019. https://jair.org/index.php/jair/article/view/11727

### Time: document dates, validity, clinical relative time

- [Chambers 2012] Chambers. Labeling Documents with Timestamps: Learning from their Time Expressions. ACL 2012. https://aclanthology.org/P12-1011/
- [Vashishth 2018b] Vashishth, Dasgupta, Ray, Talukdar. Dating Documents using Graph Convolution Networks. ACL 2018. https://aclanthology.org/P18-1149/
- [Kumar 2012] Kumar, Baldridge, Lease, Ghosh. Dating Texts without Explicit Temporal Cues. 2012. https://arxiv.org/abs/1211.2290
- [Hoffart 2013] Hoffart, Suchanek, Berberich, Weikum. YAGO2: A spatially and temporally enhanced knowledge base from Wikipedia. Artificial Intelligence 194, 2013. https://doi.org/10.1016/j.artint.2012.06.001
- [Ling 2010] Ling, Weld. Temporal Information Extraction. AAAI 2010. https://ojs.aaai.org/index.php/AAAI/article/view/7512
- [Wang 2011] Wang, Yang, Qu, Spaniol, Weikum. Harvesting Facts from Textual Web Sources by Constrained Label Propagation. CIKM 2011. https://dl.acm.org/doi/10.1145/2063576.2063698
- [Talukdar 2012] Talukdar, Wijaya, Mitchell. Coupled Temporal Scoping of Relational Facts. WSDM 2012. https://dl.acm.org/doi/10.1145/2124295.2124307
- [Reimers 2016] Reimers, Dehghani, Gurevych. Temporal Anchoring of Events for the TimeBank Corpus. ACL 2016. https://aclanthology.org/P16-1207/
- [Rasmussen 2025] Rasmussen, Paliychuk, Beauvais, Ryan, Chalef. Zep: A Temporal Knowledge Graph Architecture for Agent Memory. 2025. https://arxiv.org/abs/2501.13956
- [Lairgi 2025] Lairgi, Moncla, Benabdeslem, Cazabet, Cléau. ATOM: AdapTive and OptiMized dynamic temporal knowledge graph construction using LLMs. 2025. https://arxiv.org/abs/2510.22590
- [Soulard 2025] Soulard, Saïs, Raad. When Facts Expire: Benchmarking Temporal Validity in Knowledge Graphs. CIKM 2025. https://dl.acm.org/doi/10.1145/3746252.3761648
- [Sun 2013] Sun, Rumshisky, Uzuner. Evaluating temporal relations in clinical text: 2012 i2b2 Challenge. JAMIA 20(5), 2013. https://academic.oup.com/jamia/article/20/5/806/726374
- [Sun 2015] Sun, Rumshisky, Uzuner. Normalization of relative and incomplete temporal expressions in clinical narratives. JAMIA 22(5), 2015. https://academic.oup.com/jamia/article-abstract/22/5/1001/928475
- [Styler 2014] Styler et al. Temporal Annotation in the Clinical Domain. TACL 2, 2014. https://aclanthology.org/Q14-1012/
- [Olex 2022] Olex, McInnes. Temporal disambiguation of relative temporal expressions in clinical texts. Frontiers in Research Metrics and Analytics 7, 2022. https://www.frontiersin.org/journals/research-metrics-and-analytics/articles/10.3389/frma.2022.1001266/full
- [SEC FSDS] U.S. Securities and Exchange Commission. Financial Statement Data Sets, readme (fields `fye`, `fy`, `fp`, `period`). https://www.sec.gov/files/aqfs.pdf
- [Wikidata Help] Wikidata. Help:Statements (value, unknown value, no value); Help:Dates (precision, earliest and latest date); Property:P582 end time. https://www.wikidata.org/wiki/Help:Dates

### Time: the ledger

- [Snodgrass 1999] Snodgrass. Developing Time-Oriented Database Applications in SQL. Morgan Kaufmann, 1999. https://dl.acm.org/doi/10.5555/320037
- [Jensen 1999] Jensen, Snodgrass. Temporal Data Management. IEEE TKDE 11(1), 1999. https://www2.cs.arizona.edu/~rts/pubs/TKDEJan99.pdf
- [Jensen 1998] Jensen, Dyreson et al. The Consensus Glossary of Temporal Database Concepts, February 1998 Version. Temporal Databases: Research and Practice, LNCS 1399, 1998. https://link.springer.com/chapter/10.1007/BFb0053710
- [Kulkarni 2012] Kulkarni, Michels. Temporal features in SQL:2011. SIGMOD Record 41(3), 2012. https://dl.acm.org/doi/10.1145/2380776.2380786
- [Clifford 1997] Clifford, Dyreson, Isakowitz, Jensen, Snodgrass. On the Semantics of "Now" in Databases. ACM TODS 22(2), 1997. https://ulp.cs.arizona.edu/people/rts/pubs/TODS97.pdf
- [Torp 2000] Torp, Jensen, Snodgrass. Effective Timestamping in Databases. VLDB Journal 8, 2000. https://link.springer.com/article/10.1007/s007780050008
- [Bettini 2000] Bettini, Jajodia, Wang. Time Granularities in Databases, Data Mining, and Temporal Reasoning. Springer, 2000. https://link.springer.com/book/10.1007/978-3-662-04228-1
- [Dyreson 1998] Dyreson, Snodgrass. Supporting Valid-time Indeterminacy. ACM TODS 23(1), 1998. https://dl.acm.org/doi/10.1145/288086.288087
- [Chekol 2018] Chekol, Stuckenschmidt. Towards Probabilistic Bitemporal Knowledge Graphs. WWW 2018 Companion. https://dl.acm.org/doi/10.1145/3184558.3191637
- [Tissot 2019] Tissot, Del Fabro, Derczynski, Roberts. Normalisation of imprecise temporal expressions extracted from text. Knowledge and Information Systems 61(3), 2019. https://link.springer.com/article/10.1007/s10115-019-01338-1
- [Jarnac 2025] Jarnac, Chabot, Couceiro. Uncertainty Management in the Construction of Knowledge Graphs: A Survey. TGDK 3(1), 2025. https://drops.dagstuhl.de/entities/document/10.4230/TGDK.3.1.3
