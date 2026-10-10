-- Conflict rows and their facts can be rewritten. Keep the reviewed item and the
-- action's inputs/undo on the decision instead of reconstructing them from live rows.
ALTER TABLE agent_decisions ADD COLUMN summary TEXT;
ALTER TABLE agent_decisions ADD COLUMN detail JSONB NOT NULL DEFAULT '{}';
ALTER TABLE agent_decisions DROP CONSTRAINT agent_decisions_target_kind_check;
ALTER TABLE agent_decisions DROP CONSTRAINT agent_decisions_action_check;
ALTER TABLE agent_decisions ADD CONSTRAINT agent_decisions_target_kind_check
    CHECK (target_kind IN ('review', 'conflict'));
ALTER TABLE agent_decisions ADD CONSTRAINT agent_decisions_action_check CHECK (
    (target_kind = 'review' AND action IN ('merge', 'keep', 'unsure')) OR
    (target_kind = 'conflict' AND action IN
        ('close_old', 'retime_new', 'keep_both', 'reject_new', 'unsure'))
);

-- A rewrite gives a conflict a new ID. The two fact lineages identify the question
-- a person already reverted; the ordinary target-ID index cannot answer that lookup.
CREATE INDEX agent_decisions_conflict_pair_idx
    ON agent_decisions (kb_id, (detail->>'pair_key')) WHERE target_kind = 'conflict';

-- A withdrawal or rewrite can happen outside the governor (for example while
-- replacing a document). Retire its proposal at that write, not when someone
-- eventually opens the review page.
CREATE FUNCTION supersede_changed_conflict_proposals() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE agent_decisions SET status = 'superseded', decided_at = now()
     WHERE kb_id = OLD.kb_id AND target_kind = 'conflict'
       AND target_id = OLD.id AND status = 'proposed';
    RETURN NULL;
END;
$$;
CREATE TRIGGER conflict_proposals_follow_their_input
    AFTER UPDATE OF status, old_fact_id, new_fact_id, reason ON fact_conflicts
    FOR EACH ROW
    WHEN ((OLD.status, OLD.old_fact_id, OLD.new_fact_id, OLD.reason)
          IS DISTINCT FROM (NEW.status, NEW.old_fact_id, NEW.new_fact_id, NEW.reason))
    EXECUTE FUNCTION supersede_changed_conflict_proposals();
CREATE TRIGGER conflict_proposals_follow_deletion
    AFTER DELETE ON fact_conflicts FOR EACH ROW
    EXECUTE FUNCTION supersede_changed_conflict_proposals();
