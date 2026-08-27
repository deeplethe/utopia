-- 0007: 来源即文件夹 —— source 是 Library 中的容器，挂着它摄入的文档，可定时同步。
-- kind: upload（手动上传的虚拟归属，通常 source_id 为 NULL）| watch_folder | url | rss | api
-- 设计见 docs/DESIGN.md §4 摄入渠道。

ALTER TABLE sources ADD COLUMN config JSONB NOT NULL DEFAULT '{}';
-- NULL = 仅手动同步
ALTER TABLE sources ADD COLUMN sync_interval_minutes INTEGER;
ALTER TABLE sources ADD COLUMN last_sync_at TIMESTAMPTZ;
ALTER TABLE sources ADD COLUMN last_sync_status TEXT NOT NULL DEFAULT 'never'
    CHECK (last_sync_status IN ('never', 'queued', 'running', 'ok', 'failed'));
ALTER TABLE sources ADD COLUMN last_sync_error TEXT;
ALTER TABLE sources ADD COLUMN last_sync_added INTEGER NOT NULL DEFAULT 0;

-- 文档标签（过滤与批量组织；不做实体文件夹）
ALTER TABLE documents ADD COLUMN tags TEXT[] NOT NULL DEFAULT '{}';
CREATE INDEX documents_tags_idx ON documents USING gin (tags);
