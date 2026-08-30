-- 采纳一个实体类型时，哪些实体被改了类。
--
-- 与 fact_adoptions 对称，理由也一样：只建类型不动实体的话，本体长大了、图没变好。
-- 而改完必须能撤销，否则没人敢让系统自动建类。
--
-- 实体不是 append-only 的（它是可变行，P0 的 PATCH 就直接改），所以撤销靠记下
-- 改之前的类型，而不是靠 supersedes 链。
CREATE TABLE entity_retypes (
    batch_id     UUID NOT NULL,
    kb_id        UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    entity_id    UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    -- **可空**：0009 之后最常见的一次改类正是「从没有类到有类」，而这张表是
    -- 撤销的唯一依据。非空的话第一次赋类就写不进账，那批改动不可撤
    from_type_id UUID REFERENCES entity_types(id) ON DELETE CASCADE,
    to_type_id   UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    -- 与 fact_adoptions 一致：撤销标记而非删除，采纳发生过、撤销也发生过
    reverted_at  TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 谁改的。可空 = 引擎自动裁决，跟 `entity_merges.merged_by` 一个约定。
    --
    -- **它只回答「谁发起了这次改动」**，不回答「这个类是不是人判的」——
    -- 那两件事被混为一谈过一次：类型消解把点「运行」的人传了下来，于是每个
    -- 被引擎裁决的实体都成了 `type_source = human`，从此再不被消解（见 #117）
    actor_id     UUID REFERENCES users(id),
    PRIMARY KEY (batch_id, entity_id)
);

CREATE INDEX entity_retypes_kb_idx ON entity_retypes (kb_id, created_at DESC);
