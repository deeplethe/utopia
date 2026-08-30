-- 本体导入的第一层：原文保真。
--
-- 投影只覆盖今天能消费的那部分（类、标签、rdfs:comment、subClassOf、
-- 对象/数据属性、functional、domain/range）。**读不懂的不是错误，是"暂未投影"**——
-- 原文按内容寻址存进 blob，将来推理机上线或我们补上新消费者时重跑，
-- 用户什么都不用做。这样"我们表达不了"从能力缺口降级成"投影暂未覆盖"。
CREATE TABLE ontology_imports (
    id            UUID PRIMARY KEY,
    kb_id         UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- blob 的内容指纹；同一份文件重复导入不重复占空间
    sha256        TEXT NOT NULL,
    filename      TEXT NOT NULL,
    -- turtle | rdfxml
    format        TEXT NOT NULL,
    byte_size     BIGINT NOT NULL,
    -- 投影版本：将来投影逻辑变了，据此知道哪些导入该重跑
    projection_version INT NOT NULL DEFAULT 1,
    -- 这次投影做了什么（新建/更新/暂未投影的计数与明细），预览与事后审计共用
    summary       JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- 谁导的。删号后留 NULL，与审计台账同规矩
    imported_by   UUID REFERENCES users(id) ON DELETE SET NULL,
    imported_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ontology_imports_kb_idx ON ontology_imports (kb_id, imported_at DESC);

