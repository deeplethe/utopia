-- subClassOf 从单列改为关联表：一个类可以有多个父类。
--
-- **为什么必须多父**：真实词汇表里这是常态。FOAF 的 Person 同时是 foaf:Agent
-- 与 geo:SpatialThing——两个方向，不是同一根链上的祖孙。导入此前只投影第一个，
-- 于是 domain 落在另一支上的属性判定不过：latitude 的 domain 是 SpatialThing，
-- 而 person 只挂在 agent 下，那条事实抽出来了却被挡掉（报 attr_domain_mismatch）。
--
-- **is_primary 不是冗余**。左栏按树展示，一个类只能画在一处，而"画在哪一支下"
-- 是 subClassOf 集合本身答不出的问题——它是多出来的一条信息，不是同一件事记两遍。
-- 部分唯一索引保证每个类至多一个主父。

CREATE TABLE entity_type_parents (
    child_id   UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    parent_id  UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    -- 左栏画树时走这一支。不参与语义，只管展示
    is_primary BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (child_id, parent_id),
    -- 自环在这里就挡掉；更长的环由应用在写入前查（SQL 拦不住 A→B→A）
    CONSTRAINT entity_type_parents_no_self CHECK (child_id <> parent_id)
);

CREATE UNIQUE INDEX entity_type_parents_primary_idx
    ON entity_type_parents (child_id) WHERE is_primary;

-- 反向查：左栏要按父类找子类，域判定要沿父链上溯
CREATE INDEX entity_type_parents_parent_idx ON entity_type_parents (parent_id);

-- 回填。原来的单父就是主父——它本来就是左栏画树用的那一支
INSERT INTO entity_type_parents (child_id, parent_id, is_primary)
SELECT id, parent_id, TRUE FROM entity_types WHERE parent_id IS NOT NULL;

ALTER TABLE entity_types DROP COLUMN parent_id;
