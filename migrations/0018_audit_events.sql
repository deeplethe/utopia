-- 审计日志：谁在何时对什么做了什么（纯审计，不承载回滚等衍生功能）。
-- detail 存变更要点（jsonb，按 action 语义自定），actor 删号后置 NULL 保留事件。
CREATE TABLE IF NOT EXISTS audit_events (
    id          UUID PRIMARY KEY,
    kb_id       UUID REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    actor_id    UUID REFERENCES users(id) ON DELETE SET NULL,
    action      TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id   UUID,
    detail      JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS audit_events_kb_time_idx
    ON audit_events (kb_id, created_at DESC);
