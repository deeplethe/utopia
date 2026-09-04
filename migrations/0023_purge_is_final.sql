-- 真删（#268 下半）。删除是认知轴上的事件、可撤销（0022）；purge 是把内容真的抹掉，
-- 不可撤销，只对已删除的文档开放：原文 blob（别处不再引用的才删）、分块行（证据引文
-- 随外键级联）、版本行都没了；文档行留作墓碑打 purged_at，事实保持作废，
-- document_deletions 那条账留着——「曾经有过这么一篇，某时被删、某时被清」。
ALTER TABLE documents ADD COLUMN purged_at TIMESTAMPTZ;
-- 清掉的行让出身份：external_key 清空（同步源里若还有这篇，下次当新文档重新摄入），
-- 同一份内容也可以再传——唯一索引只管没清的行
DROP INDEX documents_kb_sha_idx;
CREATE UNIQUE INDEX documents_kb_sha_idx ON documents (kb_id, sha256) WHERE purged_at IS NULL;
