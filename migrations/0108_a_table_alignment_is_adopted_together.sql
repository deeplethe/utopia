-- 0036: one proposal owns the whole table. Columns refer to ontology properties,
-- never to concept entities; the conversion is the rule expression tree.
-- Schema proposals do not participate in the question agent's outcome statistics.
ALTER TABLE ontology_proposals DROP CONSTRAINT ontology_proposals_proposed_by_check;
ALTER TABLE ontology_proposals ADD CONSTRAINT ontology_proposals_proposed_by_check
    CHECK (proposed_by IN ('suggest', 'agent', 'exploration'));

CREATE TABLE table_alignments (
    id UUID PRIMARY KEY,
    kb_id UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    data_source_id UUID NOT NULL,
    table_name TEXT NOT NULL,
    class_id UUID NOT NULL REFERENCES entity_types(id) DEFERRABLE INITIALLY DEFERRED,
    proposal_id UUID NOT NULL REFERENCES ontology_proposals(id) DEFERRABLE INITIALLY DEFERRED,
    UNIQUE (kb_id, data_source_id, table_name),
    FOREIGN KEY (kb_id, data_source_id)
        REFERENCES kb_data_sources(kb_id, data_source_id) ON DELETE CASCADE
);

CREATE TABLE table_alignment_columns (
    alignment_id UUID NOT NULL REFERENCES table_alignments(id) ON DELETE CASCADE,
    column_name TEXT NOT NULL,
    class_id UUID NOT NULL REFERENCES entity_types(id) DEFERRABLE INITIALLY DEFERRED,
    property_id UUID NOT NULL REFERENCES relation_types(id) DEFERRABLE INITIALLY DEFERRED,
    target_class_id UUID REFERENCES entity_types(id) DEFERRABLE INITIALLY DEFERRED,
    expression JSONB,
    -- Per conversion: ontology attribute UUID -> raw input column. Two flattened
    -- owners can reuse e.g. name without making their raw readings ambiguous.
    input_columns JSONB NOT NULL DEFAULT '{}',
    PRIMARY KEY (alignment_id, column_name),
    -- A relation identifies a target; an attribute converts a raw column value.
    CHECK ((target_class_id IS NULL) = (expression IS NOT NULL))
);
