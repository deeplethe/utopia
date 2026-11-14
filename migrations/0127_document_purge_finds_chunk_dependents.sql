-- 永久删除文档时，每删一个块，外键就到这两张表里找一次引用。已有主键以
-- fact_id 打头，不能按块找；照 0094 给级联删除补与外键同形的索引。
-- 大库可按 docs/design/document-purge-index-upgrade.md 先并发建好。先查目录再决定
-- 是否执行普通 CREATE INDEX：即使有 IF NOT EXISTS，它也会先申请阻塞写入的表锁。
DO $$
BEGIN
    -- 并发建索引中断会留下同名的无效索引，不能让迁移把它记成成功。
    -- 先验证已有的两个名字，再建缺的索引，免得发现错误前白锁一张大表。
    IF EXISTS (
        SELECT 1
        FROM (VALUES
            ('public.fact_evidence_chunk_fk_idx', 'public.fact_evidence'::regclass, ARRAY['chunk_id']),
            ('public.time_mentions_chunk_fk_idx', 'public.time_mentions'::regclass, ARRAY['kb_id', 'chunk_id'])
        ) AS expected(index_name, target_table, key_columns)
        WHERE to_regclass(expected.index_name) IS NOT NULL AND NOT EXISTS (
            SELECT 1
            FROM pg_index i
            JOIN pg_class idx ON idx.oid = i.indexrelid
            JOIN pg_am am ON am.oid = idx.relam
            JOIN pg_opclass op ON op.opcmethod = am.oid
              AND op.opcnamespace = 'pg_catalog'::regnamespace AND op.opcname = 'uuid_ops'
            WHERE i.indexrelid = to_regclass(expected.index_name)
              AND i.indrelid = expected.target_table
              AND i.indisvalid AND i.indisready AND i.indislive AND NOT i.indisunique
              AND i.indpred IS NULL AND i.indexprs IS NULL
              AND am.amname = 'btree'
              AND i.indnkeyatts = cardinality(expected.key_columns)
              AND i.indnatts = cardinality(expected.key_columns)
              AND NOT EXISTS (
                  SELECT 1 FROM generate_subscripts(expected.key_columns, 1) AS keys(n)
                  WHERE pg_get_indexdef(i.indexrelid, n, false) <> expected.key_columns[n]
                     OR i.indclass[n - 1] <> op.oid OR i.indoption[n - 1] <> 0
              )
        )
    ) THEN
        RAISE EXCEPTION 'Document purge indexes are invalid or do not match the expected definitions'
            USING HINT = 'See docs/design/document-purge-index-upgrade.md before retrying the migration.';
    END IF;

    IF to_regclass('public.fact_evidence_chunk_fk_idx') IS NULL THEN
        CREATE INDEX fact_evidence_chunk_fk_idx ON public.fact_evidence (chunk_id);
    END IF;
    IF to_regclass('public.time_mentions_chunk_fk_idx') IS NULL THEN
        CREATE INDEX time_mentions_chunk_fk_idx ON public.time_mentions (kb_id, chunk_id);
    END IF;
END;
$$;
