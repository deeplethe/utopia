-- 图谱重建先删这个库的全部事实、再删全部实体（graph::purge_graph）。每删一行，
-- 外键检查都要在引用它的表里找一遍；现有索引多半是只覆盖现行事实的部分索引，
-- 外键检查不带那个条件，用不上，于是逐行全表扫——staging 上一个库的
-- DELETE FROM entities 跑了十多分钟。这里给每一条指向 facts / entities 的外键
-- 补一份与它同形的索引（复合外键以 kb_id 打头，单列外键就单列）。
-- 可空的列只索引非空的行：检查里的等号能推出非空，部分索引用得上，也小得多。
-- 与仓库里其他建索引的迁移一样不用 CONCURRENTLY：迁移在事务里跑，起服务时图还没人写。

-- facts 指向 entities / facts
CREATE INDEX IF NOT EXISTS facts_kb_subject_fk_idx ON facts (kb_id, subject_id);
CREATE INDEX IF NOT EXISTS facts_kb_object_fk_idx ON facts (kb_id, object_id);
CREATE INDEX IF NOT EXISTS facts_kb_supersedes_fk_idx ON facts (kb_id, supersedes) WHERE supersedes IS NOT NULL;

-- 实体互指、合并与裁决记录
CREATE INDEX IF NOT EXISTS entities_merged_into_fk_idx ON entities (merged_into);
CREATE INDEX IF NOT EXISTS resolution_reviews_left_fk_idx ON resolution_reviews (left_id);
CREATE INDEX IF NOT EXISTS resolution_reviews_right_fk_idx ON resolution_reviews (right_id);
CREATE INDEX IF NOT EXISTS entity_merges_source_fk_idx ON entity_merges (source_id);
CREATE INDEX IF NOT EXISTS entity_merges_target_fk_idx ON entity_merges (target_id);
CREATE INDEX IF NOT EXISTS entity_retypes_entity_fk_idx ON entity_retypes (entity_id);

-- 限定词、采纳、冲突、公理违规、概念映射
CREATE INDEX IF NOT EXISTS statement_qualifiers_entity_fk_idx ON statement_qualifiers (entity_id);
CREATE INDEX IF NOT EXISTS fact_qualifiers_entity_fk_idx ON fact_qualifiers (entity_id);
CREATE INDEX IF NOT EXISTS fact_adoptions_new_fk_idx ON fact_adoptions (new_fact_id);
CREATE INDEX IF NOT EXISTS fact_conflicts_new_fact_fk_idx ON fact_conflicts (new_fact_id);
CREATE INDEX IF NOT EXISTS axiom_violations_left_fact_fk_idx ON axiom_violations (left_fact);
CREATE INDEX IF NOT EXISTS axiom_violations_right_fact_fk_idx ON axiom_violations (right_fact);
CREATE INDEX IF NOT EXISTS concept_mappings_concept_fk_idx ON concept_mappings (concept_id);

-- 待确认 / 已拒绝 / 推出的事实（现有索引以 kb_id 打头或只覆盖未作废的）
CREATE INDEX IF NOT EXISTS pending_facts_subject_fk_idx ON pending_facts (subject_id);
CREATE INDEX IF NOT EXISTS pending_facts_object_fk_idx ON pending_facts (object_id);
CREATE INDEX IF NOT EXISTS rejected_facts_subject_fk_idx ON rejected_facts (subject_id);
CREATE INDEX IF NOT EXISTS rejected_facts_object_fk_idx ON rejected_facts (object_id);
CREATE INDEX IF NOT EXISTS derived_facts_kb_subject_fk_idx ON derived_facts (kb_id, subject_id);
CREATE INDEX IF NOT EXISTS derived_facts_kb_object_fk_idx ON derived_facts (kb_id, object_id);

-- 0073 的读数缓存、蕴含来源、勘误动作（现有部分索引只覆盖 applied 的 retract/revise）
CREATE INDEX IF NOT EXISTS phrase_readings_kb_entity_fk_idx ON phrase_readings (kb_id, entity_id);
CREATE INDEX IF NOT EXISTS implied_fact_sources_entity_fk_idx ON implied_fact_sources (entity_id);
CREATE INDEX IF NOT EXISTS errata_actions_kb_statement_fk_idx ON errata_actions (kb_id, statement_id);
CREATE INDEX IF NOT EXISTS errata_actions_kb_new_fact_fk_idx ON errata_actions (kb_id, new_fact_id) WHERE new_fact_id IS NOT NULL;
