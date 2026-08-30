-- 0005: 本体编辑器基础 —— 类型层级（subClassOf 数据面）+ 抽取未匹配统计（LLM 建议流的信号源）

-- 实体类型层级：parent_id = subClassOf（公理推理 P4 点亮；编辑器先落数据）
ALTER TABLE entity_types ADD COLUMN parent_id UUID REFERENCES entity_types(id) ON DELETE SET NULL;

-- 抽取过程中命中白名单之外的类型/关系：不是垃圾，是本体扩展的信号
CREATE TABLE ontology_misses (
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL CHECK (kind IN ('entity_type', 'relation_type')),
    key        TEXT NOT NULL,
    example    TEXT,
    count      INT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kb_id, kind, key)
);
