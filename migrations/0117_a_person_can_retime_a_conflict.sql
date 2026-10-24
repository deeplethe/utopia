-- ADR 0043's human slice: changing the successor's start is a decision of its own.
ALTER TABLE fact_conflicts DROP CONSTRAINT fact_conflicts_resolution_check;
ALTER TABLE fact_conflicts ADD CONSTRAINT fact_conflicts_resolution_check
    CHECK (resolution IS NULL OR resolution IN ('closed', 'retimed_new', 'kept_both', 'rejected_new'));
COMMENT ON COLUMN fact_conflicts.resolution IS
    'Human decisions: closed | retimed_new | kept_both | rejected_new; withdrawn has NULL (0043, 0051)';
