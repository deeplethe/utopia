-- 被挡掉的事实不留痕。抽取器有七处 `continue`：主语类型不明、属性 domain
-- 不匹配、值不合 datatype、置信度不够……事实抽出来了、被挡掉、什么都不说，
-- 用户只看到图里少了东西。与"账本 append-only、每条事实都有证据、不确定性
-- 浮到人面前"三条原则直接冲突。

-- 按 document 归集：既能算单篇的"多少条没落地"，也能在 KB 层聚合。
-- 同时天然修好生命周期——ontology_misses 只在整库重建时清（graph.rs），
-- 来源级重抽不清，会攒陈旧计数；按 document 清则每次重抽自动作数。
CREATE TABLE extraction_drops (
    kb_id       UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    document_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- 机器可聚合的原因码（attr_domain_mismatch / low_confidence / ...）
    reason      TEXT NOT NULL,
    -- 该原因下的具体对象（属性 key、谓词名、"salary@organization"）
    detail      TEXT NOT NULL,
    count       INT NOT NULL DEFAULT 1,
    -- 一个样例，让人一眼看出丢的是什么（"Acme Corp → salary"）
    example     TEXT,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kb_id, document_id, reason, detail)
);

CREATE INDEX extraction_drops_doc_idx ON extraction_drops (document_id);

