-- 0061 · The ontology is proposed from the open graph and judged by its questions (cut 1).
--
-- 一个库该回答什么问题，从前没有地方写；本体的一个元素值不值得批，也就无从判断。
-- 能力问题成为库里的行、成为判据；本体代理从开放图谱里对不上本体的签名和类别词出发，
-- 对着这些问题提案。提案落在 0003 的 `ontology_proposals` 里，多带三样东西：谁提的、
-- 它服务哪些问题、它会绑上哪些签名；被拒时记下理由，下一轮不再提同一条。
CREATE TABLE competency_questions (
    id              UUID PRIMARY KEY,
    kb_id           UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- 人的原话
    question        TEXT NOT NULL,
    -- 期望的答案（可空）；或答案要走的形状：{"classes": [...], "properties": [...]}
    expected_answer TEXT,
    needs           JSONB,
    -- person 写的，或 agent 从开放图谱里提的（proposed，等人认）
    origin          TEXT NOT NULL DEFAULT 'person' CHECK (origin IN ('person', 'agent')),
    status          TEXT NOT NULL DEFAULT 'accepted'
                    CHECK (status IN ('accepted', 'proposed', 'rejected', 'retired')),
    created_by      UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 上次按它检查本体的时间与结果（cut 3 的度量写这里）
    last_checked_at TIMESTAMPTZ,
    last_result     JSONB
);
CREATE INDEX competency_questions_kb ON competency_questions (kb_id, status);

ALTER TABLE ontology_proposals
    ADD COLUMN proposed_by TEXT NOT NULL DEFAULT 'suggest'
        CHECK (proposed_by IN ('suggest', 'agent')),
    -- 拒绝的理由（人写的）；代理下一轮不再为同一批签名提同一条
    ADD COLUMN reason      TEXT,
    -- 它服务的能力问题
    ADD COLUMN serves      UUID[] NOT NULL DEFAULT '{}',
    -- 它会绑上的形状：{"phrases": [{phrase, subject_type_id, object_type_id, object_is_value,
    -- statement_count}], "kind_words": [{kind_word, count}]}
    ADD COLUMN signatures  JSONB NOT NULL DEFAULT '{}';
