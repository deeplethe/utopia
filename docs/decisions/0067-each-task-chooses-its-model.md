# 0067 · Each task chooses its model

- **Status**: Proposed · 2026-10-15 · #470 · open: cuts 1–4
- **Written**: 2026-10-15 (conventions in the [README](README.md))
- **Related**: [#470](https://github.com/deeplethe/utopia/issues/470) (the proposal, its cuts and the six tasks); [0042](0042-the-chat-loop-is-a-runner-with-hooks.md) (a per-task model is not wired); [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) (a model trained for these steps is the planned route); [0065](0065-readers-choose-their-provider.md) (readers keep their own provider); [#1066](https://github.com/deeplethe/utopia/pull/1066) (one reasoning effort for every call); [#996](https://github.com/deeplethe/utopia/pull/996) (call labels in the usage log)

## Problem

A workspace has one `llm_settings` row, so every call goes to one chat endpoint and one embedding endpoint, and since #1066 with one reasoning effort. The calls have opposite economics. Extraction reads every chunk of every document and should be cheap and fast; adjudication and Chat should be careful. Alignment is about six tenths of a document's tokens, and the model 0044 plans to train for extraction and alignment has nowhere to plug in. Concurrency limits are already keyed by base URL and model (#48), so only the choosing is missing.

## Decisions

1. **A profile is a named endpoint**: a name, a kind (`chat` or `embedding`), a base URL, a key and a model, plus a reasoning effort on chat profiles. The key is sealed at rest like every model key. A profile is ready when it has an address and a model. "Extraction without thinking, alignment with" is two profiles of one model.
2. **Six tasks, grouped by what the calls are for.** #996's call labels stay as they are: a task chooses the endpoint, a label names the call in the usage log.

   | Task | The calls |
   |---|---|
   | `extraction` | reading a passage (`extraction_open::run_open`), time interpretation (`time_resolution::resolve_document`) |
   | `alignment` | kind-word and phrase votes (`type_alignment`, `phrase_alignment`), rule proposals (`implication::propose_rules`) |
   | `governance` | type resolution (`type_resolution`), entity adjudication (`adjudication` and the governance queue, `governance::govern`), errata (`errata`), the ontology agent (`ontology_agent`) |
   | `chat` | a conversation turn |
   | `mappings` | mapping exploration (`mappings::explore`, `mappings::describe_data`) |
   | `embedding` | every vector: chunks, names, the ontology, signatures, mappings and queries |

3. **The chain.** `model_for(kb, task)` takes the first link that is set: the base's override for the task (cut 2), the default for the task, then, for every task except `embedding`, the default for `chat`. A task with no default of its own follows `chat`, which is today's behaviour. `embedding` never falls back to a chat profile: with no embedding profile there are no vectors, as with no embedding model today. A link that is set but not ready ends the search, and the task counts as not configured, so its data never goes to an endpoint nobody chose for it.
4. **`model_for` is the only way to a model.** No call site reads the settings or builds a client. `model_for` returns the client together with the base URL and model that the concurrency gate is keyed by, so per-model limits stay as they are.
5. **The migration** turns each `llm_settings` row into at most two profiles, `chat` and `embedding`, and sets only those two defaults, so a migrated deployment makes the same calls. The chat and embedding columns then leave `llm_settings`; OCR and transcription stay there (0065).
6. **Cuts**, as #470 lays them out: (1) profiles, task defaults, the migration and `model_for` at every call site, with no endpoints and no UI, so until cut 3 a task gets a profile of its own only in the database; (2) a base's overrides; (3) Administration → Models as a table of profiles, plus the task defaults; (4) changing a base's embedding profile, which re-embeds that base.

## Not chosen

- **The issue's five tasks.** Alignment would hide inside another task, and it is the one with the volume and the planned model.
- **#996's call kinds as tasks.** Too fine, and they change with the prompts.
- **OCR and transcription as profiles.** A base URL, key and model cannot say which protocol a reader speaks (0065).

## Open questions

- **Where profiles live.** #470 puts them at the deployment level. Today each workspace has its own row, with its own keys, edited by that workspace's admins. Kept per workspace, the migration turns one row into at most two profiles, and no workspace's key or endpoint is used by another.
- **`implication::read_phrases`** is not in the list. It fills in the readings for the implication rules that alignment proposes, so it would go under `alignment`.
- **A base's unset tasks (cut 2).** As the chain stands they follow the defaults, so a base that has to stay on a local endpoint overrides every task it uses. The alternative is that a base's own `chat` override also covers its other unset tasks.
