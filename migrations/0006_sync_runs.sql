-- 0011: 来源同步运行记录 —— 每次同步一行（时间/状态/产出/错误），渠道的可审计历史。
-- 每来源仅保留最近 50 条（finish_run 时修剪）。

CREATE TABLE source_sync_runs (
    id           UUID PRIMARY KEY,
    source_id    UUID NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    started_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at  TIMESTAMPTZ,
    status       TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'ok', 'failed')),
    created_docs INTEGER NOT NULL DEFAULT 0,
    updated_docs INTEGER NOT NULL DEFAULT 0,
    error        TEXT
);
CREATE INDEX source_sync_runs_source_idx ON source_sync_runs (source_id, started_at DESC);
