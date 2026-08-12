# External API Guide

[English](external-api.md) · [简体中文](external-api.zh-CN.md)

OntoPilot exposes a versioned, read-only API for applications that consume a governed knowledge
system. It is intentionally separate from the Cookie-authenticated governance API used by the Web
application.

## Security Model

- Every credential belongs to exactly one knowledge system.
- A knowledge-system owner may create multiple named tokens for different consumers.
- Tokens carry explicit read scopes, may expire, and can be revoked independently.
- Authentication uses a SHA-256 hash. OntoPilot also stores an encrypted copy so the knowledge-system owner can explicitly reveal an active token again.
- Revoking a token deletes its encrypted copy. Legacy hash-only tokens created before this feature cannot be recovered and must be replaced.
- External routes never provide extraction, editing, review, history, membership, or token-management
  operations.

Create and revoke tokens from the knowledge system's **API Access** page. Send the token only in the
HTTP header:

```http
Authorization: Bearer opk_<public-id-prefix>_<secret>
```

Never place a token in a URL or commit it to source control.

## Scopes

| Scope | Grants |
| --- | --- |
| `ontology:read` | Ontology JSON view and TBox RDF export |
| `vocabulary:read` | SKOS schemes and concepts, controlled-term resolution, and vocabulary RDF export |
| `instances:read` | Class counts, individual search, and individual assertions |
| `query:read` | Bounded SPARQL `SELECT` and `ASK` queries over TBox + ABox + SKOS |
| `provenance:read` | Source documents, chunk identifiers, and evidence snippets on individual results |

The `provenance:read` scope is additive: an individual can be read with `instances:read`, but source
evidence is included only when the same token also has `provenance:read`.

## Base URL

Each knowledge system has a stable non-numeric public identifier:

```text
https://<host>/api/v1/knowledge-systems/<public-id>
```

The base URL reads the mutable workspace for backward compatibility. For governed production consumption, pin requests to an immutable published release:

```text
https://<host>/api/v1/knowledge-systems/<public-id>/releases/<version>
https://<host>/api/v1/knowledge-systems/<public-id>/published
```

The first URL is version-fixed. The second is an alias for the newest published release and may move when another version is published. Append `/ontology`, `/classes`, `/individuals`, `/vocabulary/...`, `/export`, or `/query` just as with the workspace API. `/manifest` returns the captured manifest. A stopped or deleted fixed service returns `410 Gone`; provisioning returns `503` with `Retry-After`.

The **API Access** page displays the complete base URL.

## Endpoints

| Method | Path | Required scope | Purpose |
| --- | --- | --- | --- |
| `GET` | `/` | any active scope | Public metadata, graph statistics, and granted scopes |
| `GET` | `/ontology` | `ontology:read` | Curated TBox JSON view |
| `GET` | `/export?fmt=turtle` | `ontology:read` | TBox as Turtle, RDF/XML, N-Triples, or JSON-LD |
| `GET` | `/vocabulary/schemes` | `vocabulary:read` | List SKOS concept schemes and vocabulary statistics |
| `GET` | `/vocabulary/concepts` | `vocabulary:read` | Search and page through controlled concepts |
| `GET` | `/vocabulary/resolve?q=<term>` | `vocabulary:read` | Resolve a preferred, alternative, or hidden label |
| `GET` | `/vocabulary/export?fmt=turtle` | `vocabulary:read` | Vocabulary as Turtle, RDF/XML, N-Triples, or JSON-LD |
| `GET` | `/classes` | `instances:read` | TBox classes with ABox individual counts |
| `GET` | `/individuals` | `instances:read` | Search and page through individuals |
| `GET` | `/individual?iri=<iri>` | `instances:read` | Individual types, attributes, and relationships |
| `POST` | `/query` | `query:read` | Read-only SPARQL query over the combined TBox + ABox dataset |

`GET /individuals` accepts `class_iri`, `q`, `limit` (maximum `200`), and `offset` query parameters.
`GET /vocabulary/concepts` accepts `scheme_iri`, `q`, `status`, `limit` (maximum `1000`), and `offset`.
`GET /vocabulary/resolve` accepts `q`, optional `language`, and `limit` (maximum `100`).

## REST Examples

```bash
export ONTOPILOT_BASE="http://localhost:8080/api/v1/knowledge-systems/<public-id>"
export ONTOPILOT_TOKEN="opk_..."

curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/ontology"

curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/individuals?q=pump&limit=20"

curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/vocabulary/resolve?q=pump&language=en"
```

Supported RDF export formats are `turtle`, `rdfxml`, `ntriples`, and `jsonld`.

## SPARQL Queries

The query endpoint treats the knowledge system's TBox, ABox, and SKOS vocabulary graphs as one default
RDF graph. OntoPilot provides the `rdf`, `rdfs`, `owl`, `xsd`, `skos`, `dcterms`, and `onto` prefixes
automatically.

```bash
curl -sS "$ONTOPILOT_BASE/query" \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "SELECT ?entity ?label WHERE { ?entity rdfs:label ?label } ORDER BY ?label",
    "max_rows": 100
  }'
```

`SELECT` responses follow the SPARQL Results JSON binding shape and add `truncated` and `max_rows`
fields. `ASK` responses return `{"head": {}, "boolean": true|false}`.

The endpoint enforces these restrictions:

- only `SELECT` and `ASK` are accepted;
- `SERVICE`, `FROM`, `GRAPH`, and all update operations are rejected;
- only the token's knowledge-system TBox, ABox, and SKOS vocabulary are in the query dataset;
- `max_rows` is capped by `EXTERNAL_QUERY_MAX_ROWS` (default `500`);
- query text is capped by `EXTERNAL_QUERY_MAX_CHARS` (default `20000`).

These controls prevent cross-knowledge-system access and accidental graph mutation. Deployments that
accept untrusted Internet traffic should additionally configure HTTPS, request-rate limits, body-size
limits, and access logging at the reverse proxy.

## Errors

| Status | Meaning |
| --- | --- |
| `400` | Invalid or unsupported query/request |
| `401` | Missing, invalid, expired, revoked, or wrong-knowledge-system token |
| `403` | Valid token without the required scope |
| `404` | Requested individual does not exist |
| `413` | SPARQL request exceeds the configured size limit |

Revocation takes effect immediately. Existing human sessions and other tokens are unaffected.
