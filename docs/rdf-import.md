# Direct RDF Import

OntoPilot can write an existing RDF document directly into a knowledge system without document
parsing, chunking, entity extraction, or an LLM request.

## Supported Syntaxes

| Syntax | Typical extensions | Import format value |
| --- | --- | --- |
| Turtle | `.ttl`, `.owl` | `turtle` |
| RDF/XML | `.rdf`, `.xml`, `.owl` | `rdfxml` |
| N-Triples | `.nt` | `ntriples` |
| JSON-LD | `.jsonld`, `.json` | `jsonld` |

`auto` detects the syntax from the extension and content. The `.owl` extension is content-sniffed
because OWL documents commonly use either Turtle or RDF/XML.

TriG and N-Quads are not accepted because one OntoPilot knowledge system already owns fixed TBox
and ABox named graphs.

## Destination

| Target | Behavior |
| --- | --- |
| `auto` | Splits OWL/RDFS schema into the TBox and remaining facts into the ABox |
| `tbox` | Writes every parsed triple to the ontology graph |
| `abox` | Writes every parsed triple to the instance graph |

Automatic splitting recognizes class, property, ontology, restriction, domain/range, class/property
relation, OWL collection, property-chain, and SHACL shape structures. Labels and custom annotations
whose subject is a recognized schema resource stay with that resource in the TBox. Named individuals
and ordinary assertions remain in the ABox.

The split is intentionally conservative and heuristic. Use an explicit target for RDF that relies on
OWL punning, undeclared schema resources, or a domain-specific metamodel.

## Write Mode

- `merge` adds missing triples and leaves existing triples untouched.
- `replace` clears the selected destination graph before adding the parsed triples.
- `replace` with `auto` replaces both the TBox and ABox graphs.

Parsing and limit checks complete before any graph is cleared. Every changed graph receives an exact
N-Triples diff in change history, so merge and replace imports can be rolled back. A dual-graph import
uses one history group and rolls back both graphs together.

Blank nodes are scoped by the knowledge system, base IRI, destination, and source file SHA-256. This
prevents unrelated imports from accidentally sharing blank nodes while keeping an exact repeat with
the same options idempotent.

## Web API

The governance API requires an authenticated human session with editor or owner access. External
knowledge-system tokens remain read-only and cannot import RDF.

```http
POST /api/knowledge/{knowledge_system_id}/rdf/import
Content-Type: multipart/form-data
```

Multipart fields:

| Field | Required | Values / default |
| --- | --- | --- |
| `file` | yes | RDF file |
| `target` | no | `auto` (default), `tbox`, `abox` |
| `strategy` | no | `merge` (default), `replace` |
| `format` | no | `auto` (default), `turtle`, `rdfxml`, `ntriples`, `jsonld` |
| `base_iri` | no | Knowledge-system base IRI; resolves relative IRIs |

The response includes parsed and assigned triple counts, net additions/removals per graph, the
refreshed ontology view, open conflicts, and ABox validation counts.

## Limits and Storage

| Environment variable | Default |
| --- | --- |
| `RDF_IMPORT_MAX_BYTES` | `26214400` (25 MiB) |
| `RDF_IMPORT_MAX_TRIPLES` | `250000` |

OntoPilot stores the imported triples, source filename, SHA-256, import options, and reversible graph
diffs. It does not retain another copy of the uploaded RDF file. Document/chunk provenance applies to
LLM-extracted knowledge; direct imports have file-level audit provenance instead.

The workbench presents the OWL subset it understands. Other valid imported triples remain in
Oxigraph and are still available through RDF export and authorized SPARQL queries.
