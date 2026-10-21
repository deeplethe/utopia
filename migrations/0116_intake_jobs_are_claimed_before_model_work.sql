-- 不调对话模型的任务先认领（#1300）。
--
-- 认领原来只按 run_at，所有种类排一条队。几百篇文档等抽取的时候，整点的 sync_source、
-- 新同步进来那批文档的 process_document 都要等前面每一个更早的 extract_document 跑完；
-- 本地模型上那是好几天——来源十个小时没同步，423 篇新文档一直 pending、搜不到。
--
-- 优先级从 kind 算出来，排任务的地方一处不用改：不调对话模型的是 0，其余是 1。
-- embed_ontology、memory_ingest 等的是嵌入闸门，process_document 可能调 OCR 或转写，
-- 都不等对话模型。名单只在这个函数里，认领的 ORDER BY 和下面的索引都读它；要改名单，
-- 在新的迁移里 CREATE OR REPLACE 它并重建索引。没有加一列：jobs 留着全部跑完的任务，
-- 加存储列要在启动迁移时重写整张表
CREATE FUNCTION job_priority(kind text) RETURNS smallint
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN kind IN ('sync_source', 'process_document', 'hydrate_rss_entry',
                              'embed_ontology', 'memory_ingest', 'build_vector_index',
                              'materialize_inferences', 'noop')
                THEN 0::smallint ELSE 1::smallint END
$$;

CREATE INDEX jobs_priority_claim_idx ON jobs (job_priority(kind), run_at) WHERE status = 'queued';
