-- 事实行上说「这一段是人改过的」（#970）：读一个实体的事实时，要知道本库哪些行被人改过区间，
-- 也就是 `fact.time_corrected` 的审计记在了哪些行上。`audit_events` 只有 (kb_id, created_at)
-- 的索引，按动作找要把这个库的审计整个读一遍——一个没人改过区间的库也一样，而实体面板和
-- 四个图谱工具每次读都要问。部分索引只收这一种动作，库里改过几行它就几行。
CREATE INDEX IF NOT EXISTS audit_events_time_corrected_idx
    ON audit_events (kb_id, target_id)
    WHERE action = 'fact.time_corrected' AND target_id IS NOT NULL;
