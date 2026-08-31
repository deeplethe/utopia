-- 摄入管道：来源、文档、分块、版本与同步记录。

-- 来源即文件夹：source 是 Library 中的容器，挂着它摄入的文档，可定时同步。
-- kind: upload（手动上传的虚拟归属，通常 source_id 为 NULL）| watch_folder | url | rss | api。
-- 设计见 docs/DESIGN.md §4 摄入渠道
CREATE TABLE sources (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL DEFAULT 'upload',
    name       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 各种源自己的配置（url 列表、rss 地址、选择器…）。形状按 kind 变，
    -- 所以是 JSONB 而不是一堆稀疏列
    config     JSONB NOT NULL DEFAULT '{}',
    -- NULL = 仅手动同步
    sync_interval_minutes INTEGER,
    last_sync_at     TIMESTAMPTZ,
    last_sync_status TEXT NOT NULL DEFAULT 'never'
                     CHECK (last_sync_status IN ('never', 'queued', 'running', 'ok', 'failed')),
    last_sync_error  TEXT,
    last_sync_added  INTEGER NOT NULL DEFAULT 0,
    icon       TEXT,
    -- cron 表达式（标准 5 段），与 sync_interval_minutes 互斥。
    -- UI 用可视化选择器构建，Advanced 模式才暴露原生表达式
    sync_cron  TEXT,
    -- **明文存，不是哈希。** 自部署威胁模型下「只看一次」是自找麻烦：
    -- 改存明文随时可查（Editor 权限专用端点）。DB 失守时文档本体早已泄露，
    -- 密钥哈希化没有额外收益；Rotate 保留应对泄露
    ingest_token TEXT
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
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 图谱抽取状态。**与摄入管道状态分离**：两段式可用，解析完就能检索，
    -- 抽取慢慢跑
    graph_status    TEXT NOT NULL DEFAULT 'none'
                    CHECK (graph_status IN ('none', 'queued', 'extracting', 'done', 'failed')),
    -- 文档标签（过滤与批量组织；不做实体文件夹）。
    --
    -- **今天四层皆空，而且是故意留着的。** 没有任何地方写它、读它、露出它——
    -- `set_document_tags` 零调用，前端连字段名都没提过。它穿过了 53 → 19 → 10
    -- 三轮迁移折叠，没有一轮有人想起它。
    --
    -- 留着不是忘了删：**标签会是这张表上唯一「人自己贴的」维度**。来源是文档
    -- 从哪来的，名字与状态是系统给的，三者都不表达「这批要脱敏」「Q3 那一包」
    -- 这种横跨来源的、只有人知道的分组。要不要有这个维度、还是让图去承担，
    -- 等外部意见再定（见 GitHub Discussions）。
    --
    -- 下一个想清理死代码的人：先去那条讨论看看有没有结论，别直接删。
    tags            TEXT[] NOT NULL DEFAULT '{}',
    -- 来源内的逻辑身份（watch_folder 相对路径 / url / rss guid / api external_id）。
    -- 摄入据此做三路判定：新增 / 变更 / 未变——内容变了原地替换文档而不是堆积
    -- 新文档，旧版本记入 document_versions
    external_key    TEXT,
    -- 目录里消失的文件打这个戳。**默认保留不删**
    missing_since   TIMESTAMPTZ,
    -- 抽取失败的原因。独立成列而不是复用 error：那一列归解析管道所有
    -- （set_status 会清空它），互不干扰
    graph_error     TEXT,
    -- 抽取任务的所有权凭证。重抽时自增即「解雇」正在跑的那个任务：它每处理完
    -- 一个分块回读一次，发现 epoch 变了就安静退出，把文档让给新任务。
    -- 单靠 graph_status 判断不可靠——接手者会把状态写回 extracting，旧任务无从分辨
    extract_epoch   INT NOT NULL DEFAULT 0
);
CREATE INDEX documents_kb_idx ON documents (kb_id, created_at DESC);
CREATE INDEX documents_tags_idx ON documents USING gin (tags);
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


CREATE UNIQUE INDEX documents_source_key_idx
    ON documents (source_id, external_key) WHERE external_key IS NOT NULL;

-- 版本回放的原料（文件 blob 内容寻址，不删）
CREATE TABLE document_versions (
    id          UUID PRIMARY KEY,
    document_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version     INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    size_bytes  BIGINT NOT NULL DEFAULT 0,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (document_id, version)
);

-- 每次同步一行（时间/状态/产出/错误），渠道的可审计历史。
-- 每来源仅保留最近 50 条（finish_run 时修剪）
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
