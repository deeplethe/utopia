-- 合并作废重复事实之前，把它的证据抄到存活的那条上（#1426）：身份来源的名字证据
-- 否则留在作废行上，存活方不再是身份实体。抄过去的 (fact_id, chunk_id) 按位成对记在这里，
-- 撤回合并只删这些，存活行原有的证据不动
ALTER TABLE entity_merges
    ADD COLUMN copied_evidence_facts  UUID[] NOT NULL DEFAULT '{}',
    ADD COLUMN copied_evidence_chunks UUID[] NOT NULL DEFAULT '{}';
