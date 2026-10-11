-- Upload previously inserted only documents. Record the retained current content,
-- at repair time: no missing historical revisions or ingestion times are invented.
-- Purge deliberately removes versions; an existing ledger is left untouched.
INSERT INTO document_versions (id, document_id, version, sha256, size_bytes, doc_time)
SELECT gen_random_uuid(), d.id, 1, d.sha256, d.size_bytes, d.doc_time
FROM documents d
WHERE d.purged_at IS NULL
  AND NOT EXISTS (SELECT 1 FROM document_versions v WHERE v.document_id = d.id);
