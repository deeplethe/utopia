# Phase 1 Release Acceptance

## Product Goal

A new user can complete **upload → extract → review → publish → export** in ten minutes with the sample corpus.

## Timed Manual Checklist

| Target | Action | Pass condition |
| --- | --- | --- |
| 0:00–2:00 | Start Docker Compose and sign in | Health checks pass; first-run guide is visible |
| 2:00–3:00 | Verify model endpoints and create a knowledge system | Knowledge-system overview opens |
| 3:00–4:00 | Upload and parse `examples/pump-operations.txt` | Document has chunks and is selectable |
| 4:00–7:00 | Run combined TBox + ABox extraction | Job completes; TBox, terminology, and instances are populated |
| 7:00–8:30 | Process all four review queues | No blocking release-quality findings remain |
| 8:30–9:15 | Create and approve a release draft, then publish | Version status is `published` |
| 9:15–10:00 | Export the complete release | Manifest, three RDF layers, and provenance files download successfully |

## Automated Gates

```bash
cd backend
pytest -q
python scripts/check_tbox_guard.py
python scripts/check_ontolearner_regression.py data/benchmarks/ontolearner-wine-official/result.json

cd ../frontend
pnpm build

cd ..
docker compose config --quiet
```

## Current Isolated Smoke Result

The deterministic demo path was executed with an isolated SQLite database and Oxigraph directory:

- release quality gate: 0 blocking findings;
- TBox snapshot: 26 statements;
- terminology snapshot: 69 statements;
- ABox snapshot: 9 statements;
- complete bundle: 10 uncompressed artifacts;
- manifest and checksum presence: passed.

Container image execution still requires a running Docker engine on the verification host.
