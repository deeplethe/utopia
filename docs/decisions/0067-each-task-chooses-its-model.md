# 0067 · Each task chooses its model

- **Status**: Accepted · 2026-10-19 · #470 · open: cuts 1–4
- **Written**: 2026-10-15 (conventions in the [README](README.md))
- **Related**: [#470](https://github.com/deeplethe/utopia/issues/470) (the proposal, its cuts and the six tasks); [0042](0042-the-chat-loop-is-a-runner-with-hooks.md) (a per-task model is not wired); [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) (a model trained for these steps is the planned route); [0065](0065-readers-choose-their-provider.md) (readers keep their own provider); [#1066](https://github.com/deeplethe/utopia/pull/1066) (one reasoning effort for every call); [#996](https://github.com/deeplethe/utopia/pull/996) (call labels in the usage log)

## Problem

A workspace has one `llm_settings` row, so every call goes to one chat endpoint and one embedding endpoint, and since #1066 with one reasoning effort. The calls have opposite economics. Extraction reads every chunk of every document and should be cheap and fast; adjudication and Chat should be careful. Alignment is about six tenths of a document's tokens, and the model 0044 plans to train for extraction and alignment has nowhere to plug in. Concurrency limits are already keyed by base URL and model (#48), so only the choosing is missing.

## Decisions

1. **A profile is a named endpoint**: a name, a kind (`chat` or `embedding`), a base URL, a key and a model, plus a reasoning effort on chat profiles. The key is sealed at rest like every model key. A profile is ready when it has an address and a model. "Extraction without thinking, alignment with" is two profiles of one model. **Profiles live per workspace**, like today's row: keys and admins are per workspace, and a deployment-level profile would let one workspace's data go to another workspace's endpoint.
2. **Six tasks, grouped by what the calls are for.** #996's call labels stay as they are: a task chooses the endpoint, a label names the call in the usage log.

   | Task | The calls |
   |---|---|
   | `extraction` | reading a passage (`extraction_open::run_open`), time interpretation (`time_resolution::resolve_document`) |
   | `alignment` | kind-word and phrase votes (`type_alignment`, `phrase_alignment`), rule proposals (`implication::propose_rules`), the readings for those rules (`implication::read_phrases`) |
   | `governance` | type resolution (`type_resolution`), entity adjudication (`adjudication` and the governance queue, `governance::govern`), errata (`errata`), the ontology agent (`ontology_agent`) |
   | `chat` | a conversation turn |
   | `mappings` | mapping exploration (`mappings::explore`, `mappings::describe_data`) |
   | `embedding` | every vector: chunks, names, the ontology, signatures, mappings and queries |

3. **The chain.** `model_for(kb, task)` takes the first link that is set: the base's override for the task (cut 2), the default for the task, then, for every task except `embedding`, the default for `chat`. A task with no default of its own follows `chat`, which is today's behaviour. `embedding` never falls back to a chat profile: with no embedding profile there are no vectors, as with no embedding model today. A link that is set but not ready ends the search, and the task counts as not configured, so its data never goes to an endpoint nobody chose for it. A base's unset tasks follow the task defaults: a base's own `chat` override covers only `chat`, so a base that has to stay on a local endpoint overrides every task it uses.
4. **`model_for` is the only way to a model.** No call site reads the settings or builds a client. `model_for` returns the client together with the base URL and model that the concurrency gate is keyed by, so per-model limits stay as they are. The one exception is the connection test, which builds a client from the profile being edited, since that profile may not be any task's link yet.
5. **The migration** turns each `llm_settings` row into at most two profiles, `chat` and `embedding`, and sets only those two defaults, so a migrated deployment makes the same calls. `chat_reasoning_effort` moves onto the migrated chat profile, and `embed_dim` onto the migrated embedding profile: an embedding profile carries its dimension, because the vector indexes are built per dimension (0035). The chat and embedding columns then leave `llm_settings`; OCR and transcription stay there (0065).
6. **Cuts**, as #470 lays them out: (1) profiles, task defaults, the migration and `model_for` at every call site, with no endpoints and no UI, so until cut 3 a task gets a profile of its own only in the database; (2) a base's overrides; (3) Administration → Models as a table of profiles, plus the task defaults; (4) changing a base's embedding profile, which re-embeds that base.

## Not chosen

- **The issue's five tasks.** Alignment would hide inside another task, and it is the one with the volume and the planned model.
- **#996's call kinds as tasks.** Too fine, and they change with the prompts.
- **Profiles at the deployment level**, as #470 first wrote it. Keys and admins are per workspace, so a shared profile would carry one workspace's data to another's endpoint.
- **A base's `chat` override covering its other unset tasks.** It would make a base's override reach tasks nobody set for that base.
- **OCR and transcription as profiles.** A base URL, key and model cannot say which protocol a reader speaks (0065).
