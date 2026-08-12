# Endpoints and examples

Append these paths to a workspace, `published` alias, or pinned release base. `/manifest` is release-only.

| Method | Path | Scope | Purpose |
| --- | --- | --- | --- |
| GET | `/` | any valid scope | Metadata, graph statistics, release, granted scopes |
| GET | `/ontology` | `ontology:read` | Structured TBox JSON |
| GET | `/export?fmt=turtle` | `ontology:read` | Export TBox RDF |
| GET | `/vocabulary/schemes` | `vocabulary:read` | SKOS schemes and statistics |
| GET | `/vocabulary/concepts` | `vocabulary:read` | Search and paginate concepts |
| GET | `/vocabulary/resolve?q=<term>` | `vocabulary:read` | Resolve preferred, alternative, or hidden labels |
| GET | `/vocabulary/export?fmt=turtle` | `vocabulary:read` | Export the SKOS vocabulary as RDF |
| GET | `/classes` | `instances:read` | Classes and instance counts |
| GET | `/individuals` | `instances:read` | Search and paginate individuals |
| GET | `/individual?iri=<iri>` | `instances:read` | Types, assertions, and optional provenance |
| POST | `/query` | `query:read` | Read-only SPARQL `SELECT` / `ASK` |
| GET | `/manifest` | `ontology:read` | Immutable release manifest |

## REST

```bash
export ONTOPILOT_BASE="http://localhost:8080/api/v1/knowledge-systems/<public-id>/releases/v1"
export ONTOPILOT_TOKEN="opk_..."

curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/ontology"
```

## SPARQL

```bash
curl -sS "$ONTOPILOT_BASE/query" \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "SELECT ?entity ?label WHERE { ?entity rdfs:label ?label } ORDER BY ?label",
    "max_rows": 100
  }'
```

SPARQL accepts `SELECT` and `ASK` only. `SERVICE`, `FROM`, `GRAPH`, and updates are rejected. Defaults are 500 rows and 20,000 query characters; responses report truncation.
