-- 删除文档是认知轴上的一个事件，不是减法（#268）。
--
-- 此前 `documents::delete` 一句 DELETE，外键把 chunks 与 fact_evidence 一并级联掉：
-- 事实留在图里、活着、却没了出处——而隐私政策写着「事实连同其出处保留」。原始文件
-- 反倒从来没删（BlobStore 没有 delete）。同步源的文档删掉之后下一次同步按 external_key
-- 找不到，建一篇新的再抽一遍，旧事实又从没作废，同一条断言落两遍。
--
-- 现在：文档打墓碑（deleted_at），分块走已有的 superseded_at，**什么内容都不清**——
-- 内容留到显式的 purge（另一条路，未做）；只作废「每条出处都已删除」的事实；这次
-- 作废了哪些事实、打标了哪些分块记在 document_deletions 里，撤销、同步复活、同内容
-- 重传复活都从那里原路读回，不多不少。形状照 entity_merges / revert_merge。
ALTER TABLE documents ADD COLUMN deleted_at TIMESTAMPTZ;
-- 文库列表与各处计数只看活的
CREATE INDEX documents_live_idx ON documents (kb_id, created_at DESC) WHERE deleted_at IS NULL;

CREATE TABLE document_deletions (
    id                UUID PRIMARY KEY,
    kb_id             UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    document_id       UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- NULL = 引擎（来源对账的批量清理）
    deleted_by        UUID REFERENCES users(id),
    -- 这次删除作废的事实与打标的分块。撤销时只复活这两份名单里的：之前就已作废的
    -- 事实、更早版本的旧分块，都不在名单里，也就不会被误救
    invalidated_facts UUID[] NOT NULL DEFAULT '{}',
    superseded_chunks UUID[] NOT NULL DEFAULT '{}',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    reverted_at       TIMESTAMPTZ
);
CREATE INDEX document_deletions_doc_idx ON document_deletions (document_id, created_at DESC);
