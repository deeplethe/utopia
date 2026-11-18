-- 人判定短语后的物化只读写库，不等模型，不能排在抽取积压后面（#1351）。
-- 优先级 0 也让它能用 #1349 留给非对话任务的两个名额。
CREATE OR REPLACE FUNCTION job_priority(kind text) RETURNS smallint
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN kind IN ('sync_source', 'process_document', 'hydrate_rss_entry',
                              'embed_ontology', 'memory_ingest', 'build_vector_index',
                              'materialize_inferences', 'materialize_typed', 'noop')
                THEN 0::smallint ELSE 1::smallint END
$$;

-- IMMUTABLE 函数改了不会更新已经存下的索引键；旧的物化任务也必须按新优先级被找到。
REINDEX INDEX jobs_priority_claim_idx;
