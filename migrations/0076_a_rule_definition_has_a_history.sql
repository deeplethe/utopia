-- 0060 · A rule's definition has a history (#912).
--
-- A business rule was one row edited in place, and a derivation pointed at the row.
-- Change a threshold and the invalidated conclusions point at a rule that now says
-- something else: the record axis kept "we concluded this, then it stopped holding"
-- and lost "under which definition". Definitions are append-only from here: every
-- edit that changes what the rule says opens a version (a full snapshot), closes the
-- previous one with a record time, and a derivation names the version it was drawn
-- under. Name, description and the enabled switch are not the definition and do not
-- open a version.
CREATE TABLE attribute_rule_versions (
    id            UUID PRIMARY KEY,
    kb_id         UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    rule_id       UUID NOT NULL REFERENCES attribute_rules(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL CHECK (seq >= 1),
    -- 整份定义：主类、结论那几格、连接谓词、条件（组、序、侧、谓词、比较、操作数）。
    -- 形状与 `business_rules::DEFINITION_SQL` 一字不差——比较「变没变」靠的是它
    definition    JSONB NOT NULL,
    recorded_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    superseded_at TIMESTAMPTZ,
    UNIQUE (rule_id, seq)
);
-- 一条规则同一时刻只有一个没关的版本，当前版本一查即得
CREATE UNIQUE INDEX attribute_rule_versions_current
    ON attribute_rule_versions (rule_id) WHERE superseded_at IS NULL;

-- 已有的规则各得版本 1，取它现在的样子，记录时间用最后一次编辑的时间
INSERT INTO attribute_rule_versions (id, kb_id, rule_id, seq, definition, recorded_at)
SELECT gen_random_uuid(), r.kb_id, r.id, 1,
       jsonb_build_object(
           'subject_type_id', r.subject_type_id,
           'conclusion', r.conclusion,
           'conclude_type_id', r.conclude_type_id,
           'conclude_predicate_id', r.conclude_predicate_id,
           'conclude_value', r.conclude_value,
           'conclude_expr', r.conclude_expr,
           'join_predicate_id', r.join_predicate_id,
           'conditions', COALESCE((SELECT jsonb_agg(jsonb_build_object(
                   'group', c.group_seq, 'seq', c.seq, 'side', c.subject_side,
                   'predicate_id', c.predicate_id, 'op', c.op, 'operand', c.operand)
                   ORDER BY c.group_seq, c.seq)
               FROM attribute_rule_conditions c WHERE c.rule_id = r.id), '[]'::jsonb)),
       r.updated_at
  FROM attribute_rules r;

-- 派生指向它凭以推出的那个版本。规则没了版本跟着没，派生也早随规则一起走了
ALTER TABLE derived_facts
    ADD COLUMN attribute_rule_version_id UUID
        REFERENCES attribute_rule_versions(id) ON DELETE CASCADE;
UPDATE derived_facts d
   SET attribute_rule_version_id = v.id
  FROM attribute_rule_versions v
 WHERE v.rule_id = d.attribute_rule_id;
CREATE INDEX derived_facts_attribute_rule_version
    ON derived_facts (attribute_rule_version_id)
    WHERE attribute_rule_version_id IS NOT NULL;
