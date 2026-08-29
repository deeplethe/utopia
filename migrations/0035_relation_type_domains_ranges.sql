-- domain / range 从单列改为关联表。
--
-- **为什么必须是多值**：OWL 里一个属性有多个 rdfs:domain 是常态
-- （works_at 的主语可能是 person 也可能是 organization），单列表达不了，
-- 导入时只能挑一个丢一个。FOAF 里就有这样的属性。
--
-- **为什么把列删掉而不是留着当"主 domain"**：留着就是同一件事有两处记录，
-- 而两处记录迟早分叉——今天已经在别处踩过一次（预览与落库各判一次 key 冲突，
-- 结果预览说了假话）。一处权威，读的人不必猜该信哪个。
--
-- range 只服务对象属性：数据属性的 range 是字面量类型，落在 relation_types.datatype 上。

CREATE TABLE relation_type_domains (
    relation_type_id UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    entity_type_id   UUID NOT NULL REFERENCES entity_types(id)   ON DELETE CASCADE,
    PRIMARY KEY (relation_type_id, entity_type_id)
);

CREATE TABLE relation_type_ranges (
    relation_type_id UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    entity_type_id   UUID NOT NULL REFERENCES entity_types(id)   ON DELETE CASCADE,
    PRIMARY KEY (relation_type_id, entity_type_id)
);

-- 反向查：本体页要按类列出它的属性，抽取要按类筛可用属性。
-- 主键覆盖了正向，反向得自己建
CREATE INDEX relation_type_domains_entity_idx ON relation_type_domains (entity_type_id);
CREATE INDEX relation_type_ranges_entity_idx  ON relation_type_ranges  (entity_type_id);

-- 回填现有属性的单个 domain。此前只有属性用得上这一列，关系一直是 NULL
INSERT INTO relation_type_domains (relation_type_id, entity_type_id)
SELECT id, domain_type_id FROM relation_types WHERE domain_type_id IS NOT NULL;

ALTER TABLE relation_types DROP COLUMN domain_type_id;
