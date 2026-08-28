-- 采纳一个实体类型时，哪些实体被改了类。
--
-- 与 fact_adoptions 对称，理由也一样：只建类型不动实体的话，本体长大了、图没变好——
-- 那些提议过 model 的实体会继续挂在 concept 下。而改完必须能撤销，否则没人敢让
-- 系统自动建类。
--
-- 实体不是 append-only 的（它是可变行，P0 的 PATCH 就直接改），所以撤销靠记下
-- 改之前的类型，而不是靠 supersedes 链。
CREATE TABLE entity_retypes (
    batch_id     UUID NOT NULL,
    kb_id        UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    entity_id    UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    from_type_id UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    to_type_id   UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    -- 与 fact_adoptions 一致：撤销标记而非删除，采纳发生过、撤销也发生过
    reverted_at  TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (batch_id, entity_id)
);

CREATE INDEX entity_retypes_kb_idx ON entity_retypes (kb_id, created_at DESC);
