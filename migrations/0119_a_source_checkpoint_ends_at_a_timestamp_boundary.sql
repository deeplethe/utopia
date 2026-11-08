-- 不从旧的 ok 审计记录回填：旧版本可能只读完前 200 条就记成功。
-- GitHub/Jira 从 NULL 开始恢复扫描，保留全部文档、版本、任务与审计历史。
-- Custom 的批次推进由 #1397 单独实现，这里不改变它的游标。
ALTER TABLE sources ADD COLUMN sync_checkpoint timestamptz;
