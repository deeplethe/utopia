# API authentication and URLs

Create named tokens on the knowledge system's **API Access** page. The external API is read-only and separate from the web governance session.

```http
Authorization: Bearer opk_<public-id-prefix>_<secret>
```

Use a separate token per client. Never place tokens in URLs or commit them to source control.

## Scopes

| Scope | Permission |
| --- | --- |
| `ontology:read` | Ontology JSON and TBox RDF |
| `vocabulary:read` | SKOS schemes, concepts, term resolution, and export |
| `instances:read` | Class statistics, individuals, and assertions |
| `query:read` | Bounded read-only SPARQL `SELECT` / `ASK` |
| `provenance:read` | Attach documents, chunks, and evidence to instance results |

`provenance:read` augments `instances:read`; it does not grant individual access by itself.

## Base URLs

Mutable workspace, useful for internal tools:

```text
https://<host>/api/v1/knowledge-systems/<public-id>
```

Immutable release, recommended for production:

```text
https://<host>/api/v1/knowledge-systems/<public-id>/releases/<version>
```

Latest-release alias:

```text
https://<host>/api/v1/knowledge-systems/<public-id>/published
```

Pin `/releases/<version>` whenever reproducibility matters. Swagger is available at [`/api/docs`](/api/docs), with the schema at [`/api/openapi.json`](/api/openapi.json).
