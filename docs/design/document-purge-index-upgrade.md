# Adding the document purge indexes to a busy database

Migration 0127 adds two B-tree indexes to speed up the foreign-key cascades when a document is permanently deleted. If it builds the indexes itself, PostgreSQL allows reads but blocks writes to the affected tables until the migration commits. Stopping one app replica does not stop writers on other replicas.

If the database must keep accepting writes, use the migration role (the table owner) to run the following two statements **separately, in autocommit, before starting the upgrade**. Run one statement at a time; do not send them in a single `psql -c` or transaction. Concurrent builds allow normal writes, but do extra I/O and can wait for long-running transactions.

```sql
CREATE INDEX CONCURRENTLY fact_evidence_chunk_fk_idx ON public.fact_evidence (chunk_id);
```

```sql
CREATE INDEX CONCURRENTLY time_mentions_chunk_fk_idx ON public.time_mentions (kb_id, chunk_id);
```

If either index already exists, check it before issuing that statement. The following query should show both expected definitions and `ready` and `valid` both true. Once both indexes are ready, deploy normally; the migration validates them without requesting a regular index build lock.

```sql
SELECT i.indexrelid::regclass AS index_name, i.indisready AS ready, i.indisvalid AS valid,
       pg_get_indexdef(i.indexrelid) AS definition
FROM pg_index i
WHERE i.indexrelid IN (to_regclass('public.fact_evidence_chunk_fk_idx'),
                       to_regclass('public.time_mentions_chunk_fk_idx'));
```

An interrupted concurrent build can leave an invalid index. After confirming that no build of that index is still running and that it is the failed index, drop **only that invalid index** with a standalone `DROP INDEX CONCURRENTLY public.<index_name>` and rerun its build. A valid index with a different definition needs operator review before the upgrade. The migration rejects either case and does not drop indexes or mark itself applied.
