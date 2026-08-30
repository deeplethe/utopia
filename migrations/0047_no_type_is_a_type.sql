-- 「还没判出来」不再是一个类，见 docs/decisions/0009。
--
-- 此前它是一行叫 concept 的哨兵。哨兵有名字，有名字就会撞——SKOS 的
-- skos:Concept 派生出的 key 正是 concept，而哨兵没有 IRI，导入逻辑
-- 「占位者没有 IRI 就认领它」会让它接管哨兵：所有未分类的实体一夜之间
-- 变成正经的 skos:Concept。不是撞名被跳过，是语义被静默改写。
--
-- NULL 没有名字，撞不着；也忘不掉——漏过滤一个哨兵不会有任何提示，
-- 漏处理一个 NULL 会当场报出来。

ALTER TABLE entities ALTER COLUMN type_id DROP NOT NULL;

-- 挂在内置类上的实体一律回到未分类。
--
-- **不做"有实体就保留那个类"的挽救**：仓库至今没有 release，不存在要照顾的
-- 生产部署，而留一半内置类会让迁完的状态不确定——测试时分不清某个 organization
-- 是内置残留还是 schema.org 导进来的。宁可状态干净。
--
-- 顺序要紧：type_id 是 ON DELETE RESTRICT，不先解引用就删不掉类型行。
UPDATE entities SET type_id = NULL
WHERE type_id IN (SELECT id FROM entity_types WHERE builtin);

-- 关联表同理：属性的 domain 指向内置类时，那条关联也一并去掉。
-- 属性本身留着——它可能还挂在别的类上，而且描述与 IRI 都是有价值的
DELETE FROM relation_type_domains WHERE entity_type_id IN (SELECT id FROM entity_types WHERE builtin);
DELETE FROM relation_type_ranges  WHERE entity_type_id IN (SELECT id FROM entity_types WHERE builtin);
DELETE FROM entity_type_parents
WHERE parent_id IN (SELECT id FROM entity_types WHERE builtin)
   OR child_id  IN (SELECT id FROM entity_types WHERE builtin);

DELETE FROM entity_types WHERE builtin;

-- 唯一索引里 type_id 现在可能是 NULL，而 Postgres 里 NULL <> NULL——
-- 两个都没类型的同名实体不会被它拦住，可以并存。
--
-- **这是要的行为**：该索引的意图是「同类同名允许重复」（见 0001 P0 的两个张伟）。
-- 未分类时我们对它们是不是同一个东西知道得更少，更没有理由合并。
-- 写在这里是因为半年后看到「未分类的同名实体可以重复」会像个 bug。

-- 改类账本的起点也可能为空：类型消解现在最常见的动作正是「从没有类到有类」，
-- 而 entity_retypes 是它的撤销依据。这一列非空的话，第一次赋类就写不进账，
-- 于是那批改动不可撤——比不记账更糟
ALTER TABLE entity_retypes ALTER COLUMN from_type_id DROP NOT NULL;
