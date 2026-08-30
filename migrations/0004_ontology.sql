-- 抽取未匹配统计：LLM 建议流的信号源

-- 抽取过程中命中白名单之外的类型/关系：不是垃圾，是本体扩展的信号
CREATE TABLE ontology_misses (
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- attribute_type 与 relation_type 分开：词表外的谓词带着字面值时（
    -- `founding_date: "2015"`），缺的是一个属性而不是一个关系，记错了会让
    -- 本体提案去建一条关系
    kind       TEXT NOT NULL
               CHECK (kind IN ('entity_type', 'relation_type', 'attribute_type')),
    key        TEXT NOT NULL,
    example    TEXT,
    count      INT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 人说过不要。**标记而不是删除**：dismiss 从前是 DELETE，下一次抽取遇到
    -- 同一个词原样插回来——用户的「不要」活不过一轮抽取。自动扩本体开着时，
    -- 那等于系统覆盖人的明确决定
    dismissed_at TIMESTAMPTZ,
    PRIMARY KEY (kb_id, kind, key)
);
