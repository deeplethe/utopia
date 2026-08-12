# Releases and Exports

## Lifecycle

1. **Draft** captures an immutable TBox, terminology, ABox, and provenance snapshot.
2. **Reviewed** means the project quality gate passed and an editor approved the snapshot.
3. **Published** marks the reviewed snapshot as an official downstream version.
   Publishing automatically provisions a queryable read-only projection in the serving RDF store.
4. **Restore** replaces the mutable three-layer workspace with any ready snapshot and restores its provenance rows.

## Release-as-a-Service

Every published release receives a fixed, token-authenticated endpoint:

```text
/api/v1/knowledge-systems/{public-id}/releases/{version}
```

The endpoint serves ontology, vocabulary, instances, release-fixed provenance, RDF exports, and bounded read-only SPARQL from three dedicated named graphs in a separate Oxigraph database. The mutable workspace is never queried by a fixed release endpoint.

`/api/v1/knowledge-systems/{public-id}/published` is a non-immutable alias for the most recently published version. Pinned version responses include an ETag derived from the release manifest and an immutable private cache policy.

A service deployment can be stopped without deleting the release and can later be rebuilt from its artifacts. Deleting a release is terminal: its service graphs, provenance index, artifacts, and release-bound exports are removed. A tombstone row and audit event remain, so the version string cannot be reused and fixed endpoints return `410 Gone`.

## Semantic Diff

Diff compares RDF statement identities, not source-file order. Results are grouped by TBox, terminology, and ABox and report added/removed counts plus bounded samples.

## Export Jobs

Exports are asynchronous and may target the current workspace or an immutable release. Supported layers are:

- `tbox`
- `vocabulary`
- `abox`
- `bundle`

A bundle includes all three RDF layers, provenance JSONL, and `manifest.json`.

## Distribution

Artifacts use uncompressed N-Quads. Serve them from a shared filesystem, object store, or CDN. Keep the manifest beside its files and verify SHA-256 before ingestion.

The application stores release and export metadata in PostgreSQL, not the artifact bodies. Artifact storage is mounted under the backend data volume by default.
