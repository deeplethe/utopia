-- 人认可过的"粗类 → 细类"配对。认可一次，之后同一对不再进人工。
--
-- 为什么需要它：待人工那一档由"选中的类在不在粗类子树里"触发，而实测那条
-- 判据测的往往不是风险，是**种子类跟导入词汇表的分类树连没连上**。
-- schema.org 的 Place 另起了 place 这个 key，内置 location 一个子类都没有，
-- 于是每一次 location → city 都算跨轴——24 个实体里报了 14 条，条条正确。
--
-- 跨轴是 (粗类, 目标类) 这一对的属性，不是实体的属性。人看过一次
-- "location 下面的东西可以是 city"，第二个城市就不该再问一遍。
CREATE TABLE type_refinement_pairs (
    kb_id       uuid NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    from_type_id uuid NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    to_type_id  uuid NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    approved_by uuid REFERENCES users(id),
    approved_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (kb_id, from_type_id, to_type_id)
);
