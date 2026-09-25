-- 0047 · A rule may conclude a relation (#818).
--
-- Until now a business rule could name only its subject. The join predicate is
-- the one declared edge X --join--> Y that brings a second entity into the body;
-- the relation conclusion then lands on that same pair. One hop keeps the
-- conclusion explainable and lets a second rule carry the path further.
ALTER TABLE attribute_rules
    ADD COLUMN join_predicate_id UUID;

ALTER TABLE attribute_rule_conditions
    ADD COLUMN subject_side TEXT NOT NULL DEFAULT 'x'
        CONSTRAINT attribute_rule_condition_subject_side_check
        CHECK (subject_side IN ('x', 'y'));

ALTER TABLE attribute_rules
    ADD CONSTRAINT attribute_rules_join_predicate_same_kb
    FOREIGN KEY (kb_id, join_predicate_id)
    REFERENCES relation_types (kb_id, id)
    ON DELETE CASCADE;

ALTER TABLE attribute_rules
    DROP CONSTRAINT attribute_rules_conclusion_check,
    ADD CONSTRAINT attribute_rules_conclusion_check
        CHECK (conclusion IN ('typing', 'attribute', 'computed', 'relation'));

-- A relation needs both halves of the edge it creates. The older conclusions
-- remain single-subject and therefore keep the join predicate empty.
ALTER TABLE attribute_rules
    DROP CONSTRAINT attribute_rule_conclusion_shape,
    ADD CONSTRAINT attribute_rule_conclusion_shape CHECK (
        (conclusion = 'typing'
             AND conclude_type_id IS NOT NULL
             AND conclude_predicate_id IS NULL
             AND conclude_value IS NULL
             AND conclude_expr IS NULL
             AND join_predicate_id IS NULL)
        OR
        (conclusion = 'attribute'
             AND conclude_type_id IS NULL
             AND conclude_predicate_id IS NOT NULL
             AND conclude_value IS NOT NULL
             AND conclude_expr IS NULL
             AND join_predicate_id IS NULL)
        OR
        (conclusion = 'computed'
             AND conclude_type_id IS NULL
             AND conclude_predicate_id IS NOT NULL
             AND conclude_value IS NULL
             AND conclude_expr IS NOT NULL
             AND join_predicate_id IS NULL)
        OR
        (conclusion = 'relation'
             AND conclude_type_id IS NULL
             AND conclude_predicate_id IS NOT NULL
             AND conclude_value IS NULL
             AND conclude_expr IS NULL
             AND join_predicate_id IS NOT NULL)
    );
