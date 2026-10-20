-- 0061: no binding is an expectation, not a synthetic ontology property.
ALTER TABLE ontology_regression_cases
    ALTER COLUMN expected_property_id DROP NOT NULL,
    ALTER COLUMN expected_direction DROP NOT NULL,
    ADD CONSTRAINT ontology_regression_cases_complete_target
        CHECK ((expected_property_id IS NULL) = (expected_direction IS NULL));

-- The existing unique constraint still deduplicates bound expectations. NULLs need
-- their own uniqueness rule, including concurrent confirmations of the same case.
CREATE UNIQUE INDEX ontology_regression_cases_none
    ON ontology_regression_cases(kb_id, statement_id)
    WHERE expected_property_id IS NULL;
