-- 0008: 文档逻辑身份 —— 摄入的三路判定（新增 / 变更 / 未变）。
-- external_key = 来源内的逻辑身份（watch_folder 相对路径 / url / rss guid / api external_id）；
-- 内容变更时原地替换文档（不再堆积新文档），旧版本记入 document_versions（版本回放的原料，
-- 文件 blob 内容寻址不删）。目录里消失的文件标记 missing_since（默认保留不删）。

ALTER TABLE documents ADD COLUMN external_key TEXT;
ALTER TABLE documents ADD COLUMN missing_since TIMESTAMPTZ;
CREATE UNIQUE INDEX documents_source_key_idx
    ON documents (source_id, external_key) WHERE external_key IS NOT NULL;

CREATE TABLE document_versions (
    id          UUID PRIMARY KEY,
    document_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version     INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    size_bytes  BIGINT NOT NULL DEFAULT 0,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (document_id, version)
);
