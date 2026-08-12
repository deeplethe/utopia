# Product overview

OntoPilot is a self-hosted ontology governance workbench. It turns PDF, Word, spreadsheet, Markdown, CSV, and text sources into knowledge systems that are **reviewable, traceable, versioned, and stable to consume**.

## Why OntoPilot exists

Language models can propose structure from text, but production ontologies still require boundary control, source evidence, human decisions, version semantics, and dependable delivery. OntoPilot treats model output as governed proposals rather than final truth.

- **Governance first:** role critics, deterministic guards, and reviewers control admission.
- **Three separate layers:** TBox, SKOS vocabulary, and ABox are stored separately and governed together.
- **Evidence first:** statements link back to documents, chunks, source spans, jobs, and prompt snapshots.
- **Immutable releases:** applications pin to snapshots while engineers continue editing the workspace.
- **Self-hosted:** relational metadata, RDF graphs, source files, and artifacts stay in the deployment by default.

## Workspace versus release

| Surface | Primary users | Mutable | Purpose |
| --- | --- | --- | --- |
| Workspace | Owner, Editor, Viewer | Yes | Extract, edit, review, validate |
| Pinned release | Applications and data services | No | REST, RDF, and SPARQL |
| `published` alias | Clients following the latest release | Alias moves | Latest-release consumption |

Quality gates and review queues separate exploration from production consumption, preventing unresolved proposals from leaking into downstream applications.
