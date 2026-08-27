-- 0001: 核心多租户表 + 任务队列
-- pgvector 扩展提前建好（P1 的 chunks.embedding 依赖），使用 pgvector/pgvector 镜像自带
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE organizations (
    id          UUID PRIMARY KEY,
    name        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id            UUID PRIMARY KEY,
    org_id        UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    email         TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE workspaces (
    id          UUID PRIMARY KEY,
    org_id      UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE memberships (
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    role         TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'editor', 'viewer')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, workspace_id)
);
CREATE INDEX memberships_workspace_idx ON memberships (workspace_id);

CREATE TABLE knowledge_bases (
    id           UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL DEFAULT 'knowledge' CHECK (kind IN ('knowledge', 'memory')),
    description  TEXT,
    -- 部署的公共默认空间（workspace 里第一个建的库）：永远 open、不可删（API 强制）
    is_default   BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX knowledge_bases_workspace_idx ON knowledge_bases (workspace_id);

-- 任务队列：FOR UPDATE SKIP LOCKED 消费，见 docs/DESIGN.md 第 2 节
CREATE TABLE jobs (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind         TEXT NOT NULL,
    payload      JSONB NOT NULL DEFAULT '{}',
    status       TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'done', 'failed')),
    attempts     INT NOT NULL DEFAULT 0,
    max_attempts INT NOT NULL DEFAULT 3,
    run_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_at    TIMESTAMPTZ,
    last_error   TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX jobs_claim_idx ON jobs (run_at) WHERE status = 'queued';
