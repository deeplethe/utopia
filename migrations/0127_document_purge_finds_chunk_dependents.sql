-- 永久删除文档时，每删一个块，外键都要在这两张表里找引用；已有索引不能按块找。
-- 按 0094 的惯例给级联删除补与外键同形的索引。与 0094、0122 一样不用
-- CONCURRENTLY：迁移在事务里跑，起服务时图还没人写。
CREATE INDEX IF NOT EXISTS fact_evidence_chunk_fk_idx ON fact_evidence (chunk_id);
CREATE INDEX IF NOT EXISTS time_mentions_chunk_fk_idx ON time_mentions (kb_id, chunk_id);
