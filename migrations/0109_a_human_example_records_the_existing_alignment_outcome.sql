-- 0061 cut 2 第一片：人留的例子比较已有对齐结果，不另起模型或调度器。
-- 原陈述、目标属性属于同一个库；删掉它们时例子也消失，避免悬空预期。
CREATE TABLE ontology_regression_cases (
    id UUID PRIMARY KEY,
    kb_id UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    statement_id UUID NOT NULL,
    expected_property_id UUID NOT NULL,
    expected_direction TEXT NOT NULL CHECK (expected_direction IN ('forward', 'reverse')),
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    origin TEXT NOT NULL CHECK (origin IN ('adoption', 'person')),
    last_checked_at TIMESTAMPTZ,
    last_result JSONB,
    FOREIGN KEY (kb_id, statement_id) REFERENCES facts(kb_id, id) ON DELETE CASCADE,
    FOREIGN KEY (kb_id, expected_property_id) REFERENCES relation_types(kb_id, id) ON DELETE CASCADE,
    UNIQUE (kb_id, statement_id, expected_property_id, expected_direction)
);
CREATE INDEX ontology_regression_cases_property
    ON ontology_regression_cases(kb_id, expected_property_id);
CREATE TRIGGER ontology_regression_cases_keep_their_kb
    BEFORE UPDATE OF kb_id ON ontology_regression_cases
    FOR EACH ROW EXECUTE FUNCTION kb_ownership_is_not_reassigned();

