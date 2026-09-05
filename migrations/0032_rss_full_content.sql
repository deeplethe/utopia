-- Full-content RSS activation and hydration ledger.
-- Documents remain the authority for accepted content; these tables retain discovery,
-- baseline, retry, and terminal state after an entry leaves the feed window.

-- Deletion events own the identity released by purge. This is historical input,
-- not a second document lifecycle; restore deliberately retains it.
ALTER TABLE document_deletions ADD COLUMN external_key TEXT;
UPDATE document_deletions dd SET external_key = d.external_key
  FROM documents d WHERE d.id = dd.document_id;
CREATE INDEX document_deletions_external_key_idx
    ON document_deletions(external_key, document_id) WHERE external_key IS NOT NULL;

ALTER TABLE sources ADD COLUMN rss_generation INTEGER NOT NULL DEFAULT 0 CHECK (rss_generation >= 0),
                    ADD COLUMN rss_baselined_at TIMESTAMPTZ;

CREATE TABLE rss_full_content_entries (
    id                    UUID PRIMARY KEY,
    source_id             UUID NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    activation_generation INTEGER NOT NULL CHECK (activation_generation >= 1),
    external_key          TEXT NOT NULL,
    title                 TEXT NOT NULL,
    article_url           TEXT,
    summary               TEXT NOT NULL DEFAULT '',
    embedded_html         TEXT,
    doc_time              TIMESTAMPTZ,
    entry_kind            TEXT NOT NULL CHECK (entry_kind IN ('baseline','candidate','no_source')),
    current_job_id        BIGINT REFERENCES jobs(id) ON DELETE SET NULL,
    observed_at           TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    first_seen_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (source_id, activation_generation, external_key),
    CHECK (octet_length(external_key) <= 4096),
    CHECK (length(title) > 0 AND octet_length(title) <= 2048),
    CHECK (article_url IS NULL OR octet_length(article_url) <= 8192),
    CHECK (octet_length(summary) <= 16384),
    CHECK (embedded_html IS NULL OR octet_length(embedded_html) <= 2097152)
);
CREATE INDEX rss_full_content_entries_pending_idx
    ON rss_full_content_entries(source_id, activation_generation, first_seen_at)
    WHERE entry_kind='candidate' AND current_job_id IS NULL;
