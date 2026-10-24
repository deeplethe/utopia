-- Confidence is evidence for a decision, not an encoding of mapping status (0011).
ALTER TABLE knowledge_bases
    ADD COLUMN auto_confirm_mappings boolean NOT NULL DEFAULT false;
ALTER TABLE concept_mappings
    ADD COLUMN confidence double precision CHECK (confidence >= 0 AND confidence <= 1),
    ADD COLUMN rationale text;
