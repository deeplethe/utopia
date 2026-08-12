# Contributing to OntoPilot

Thank you for helping build an open, reliable ontology-governance system.

Participation in this project is governed by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Before You Start

- Use GitHub Issues for reproducible bugs, focused feature proposals, and design discussion.
- Do not include confidential documents, credentials, model responses, or production graph data.
- Keep changes scoped. Refactors unrelated to the reported problem should be separate pull requests.

## Development Setup

```bash
git clone https://github.com/deeplethe/ontopilot.git
cd ontopilot/backend
python -m venv .venv
source .venv/bin/activate  # PowerShell: .venv\Scripts\Activate.ps1
pip install -r requirements-dev.txt
cp .env.example .env
```

```bash
cd ../frontend
corepack enable
pnpm install --frozen-lockfile
```

PostgreSQL is used by Docker Compose. Local backend development can omit `DATABASE_URL` and use SQLite.

## Required Checks

```bash
cd backend
pytest -q
python scripts/check_tbox_guard.py

cd ../frontend
pnpm lint
pnpm build

cd ..
docker compose config --quiet
```

## Ontology Boundary Changes

Changes to extraction, role classification, terminology mapping, hierarchy repair, or validation must:

1. remain domain-neutral;
2. avoid benchmark-specific entity lists or fixed corpus filters;
3. preserve exact source grounding;
4. add or update cases under `backend/tests/gold/`;
5. pass `scripts/check_tbox_guard.py`;
6. include an OntoLearner result when the taxonomy protocol is affected.

Gold-set updates should explain why the expected role is TBox, ABox, terminology, or literal data.

## Release and Provenance Compatibility

The release manifest, N-Quads shard naming, and provenance JSONL are public interchange formats. Pull requests that change them must:

- document the schema change;
- preserve old release readability or provide a migration path;
- add regression tests;
- increment the manifest schema identifier when compatibility breaks.

## Pull Requests

- Describe the problem and root cause.
- List the validation commands you ran.
- Include screenshots for user-interface changes.
- Update English and Chinese interface copy together.
- Update documentation when behavior or configuration changes.
- Do not commit `.env`, runtime data, benchmark caches, generated exports, or credentials.

By submitting a contribution, you agree that it is licensed under Apache License 2.0.
