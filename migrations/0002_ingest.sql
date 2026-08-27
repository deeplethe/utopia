-- 0002: 摄入管道（来源/文档/分块）+ 工作区 LLM 设置

CREATE TABLE sources (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL DEFAULT 'upload',
    name       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sources_kb_idx ON sources (kb_id);

CREATE TABLE documents (
    id              UUID PRIMARY KEY,
    kb_id           UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    source_id       UUID REFERENCES sources(id) ON DELETE SET NULL,
    filename        TEXT NOT NULL,
    mime            TEXT NOT NULL DEFAULT 'application/octet-stream',
    size_bytes      BIGINT NOT NULL DEFAULT 0,
    sha256          TEXT NOT NULL,
    -- pending → parsing → indexing → embedding → ready | failed
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'parsing', 'indexing', 'embedding', 'ready', 'failed')),
    error           TEXT,
    -- 文档时间：可信分级 + 可修改（见 DESIGN.md 4.2）
    doc_time        TIMESTAMPTZ,
    doc_time_source TEXT NOT NULL DEFAULT 'file_mtime',
    text_len        INT NOT NULL DEFAULT 0,
    chunk_count     INT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX documents_kb_idx ON documents (kb_id, created_at DESC);
CREATE UNIQUE INDEX documents_kb_sha_idx ON documents (kb_id, sha256);

CREATE TABLE chunks (
    id           UUID PRIMARY KEY,
    kb_id        UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    document_id  UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    seq          INT NOT NULL,
    text         TEXT NOT NULL,
    heading      TEXT,
    char_start   INT NOT NULL DEFAULT 0,
    char_end     INT NOT NULL DEFAULT 0,
    -- 维度不定（随所选 embedding 模型），P1 顺扫检索；量大后按已配维度建 HNSW 索引
    embedding    vector,
    -- 版本软删除：文档更新时旧分块打标（superseded_at）而非物理删除——
    -- fact_evidence 引用不断链、旧版原文可回放；打标时 embedding 清空（旧版不参与检索）
    doc_version   INT NOT NULL DEFAULT 1,
    superseded_at TIMESTAMPTZ,
    -- 图谱抽取完成标记：文档更新时被"认领"的未变分块携带它跳过重抽（增量抽取），
    -- 也让中断的抽取可断点续跑
    extracted_at  TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX chunks_document_idx ON chunks (document_id, seq);
CREATE INDEX chunks_kb_idx ON chunks (kb_id);
CREATE INDEX chunks_live_idx ON chunks (document_id) WHERE superseded_at IS NULL;

-- 工作区级 LLM 设置（对话与 embedding 分开配置，OpenAI 兼容协议）
CREATE TABLE llm_settings (
    workspace_id   UUID PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    chat_base_url  TEXT,
    chat_api_key   TEXT,
    chat_model     TEXT,
    embed_base_url TEXT,
    embed_api_key  TEXT,
    embed_model    TEXT,
    embed_dim      INT,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
