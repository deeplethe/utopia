-- 词表外的谓词带着字面值时，记的是"缺一个属性"而不是"缺一个关系"。
--
-- 抽取遇到未知谓词 + 字面值宾语（`founding_date: "2015"`），从前要么整条丢掉
-- （value 有而 object 空），要么给 2015 编一个 concept 实体。现在值落进
-- object_value 挂在兜底谓词上，原词进 fact_evidence.proposed_predicate。
-- 这里补的是**给人看的那一栏**：把它记成 relation_type 会让本体提案去建一个
-- 关系，而它要的是一个属性。
ALTER TABLE ontology_misses DROP CONSTRAINT ontology_misses_kind_check;
ALTER TABLE ontology_misses ADD CONSTRAINT ontology_misses_kind_check
    CHECK (kind IN ('entity_type', 'relation_type', 'attribute_type'));
