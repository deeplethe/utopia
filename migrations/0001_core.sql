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
    -- **唯一性只约束在职账号**，见下面那个部分索引。停用之后地址就放开了——
    -- 否则「停用」等于「这个邮箱永久报废」，同一个人回来都建不了新账号
    email         TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 单租户部署里的系统管理员。第一个注册的人自动是（见 accounts.rs）
    is_admin      BOOLEAN NOT NULL DEFAULT FALSE,
    -- **软删除，不是 DELETE。** 审计事件、合并日志、改类账本、口径确认的
    -- `actor_id` 都指着这个人，而那些是审计材料——人走了仍然要能回答
    -- 「当时是谁做的」。停用只断访问：`find_user_by_email` 与
    -- `find_user_by_id` 各带一句 `deactivated_at IS NULL`，前者挡登录、
    -- 后者挡已签发的 token（会话校验走它，所以停用立即生效）
    deactivated_at TIMESTAMPTZ,
    -- 谁停的。裸外键——停用者自己也可能被停用，而那条记录还得在
    deactivated_by UUID REFERENCES users(id)
);

-- email 唯一，**但只管在职的**。停用过的账号里可以有重复地址，
-- 所以按 email 找人的查询必须带 `deactivated_at IS NULL`——它本来就要带
-- （不然停用的人还能登录），这里让那一句同时也是正确性的保证。
CREATE UNIQUE INDEX users_email_active_idx ON users (email) WHERE deactivated_at IS NULL;

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
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- open 库无矩阵记录时按部署角色行事，restricted 库仅 kb_members 里的成员
    -- 可见（外人 NotFound）
    visibility   TEXT NOT NULL DEFAULT 'open'
                 CHECK (visibility IN ('open', 'restricted')),
    -- 要不要替人扩本体。**显式开关而不是从行为里推断**：从前靠「本体有没有被
    -- 碰过」来判断，推错了会很荒唐——在提案上点一次 Add 就永久关掉建议功能，
    -- 因为那记了一条带操作人的本体动作。而且它一旦为假就永不再真，本体被冻结在
    -- 第一批文档碰巧包含的词汇上，可来源是每天持续进文档的
    auto_extend_ontology BOOLEAN NOT NULL DEFAULT TRUE,
    -- **不是「系统语言」**（见 docs/decisions/0004）。界面语言在客户端，后端没有
    -- locale。这一列管的是**语料的语言**：类的 description 逐字进抽取提示词，
    -- 读者是正在读你文档的模型——描述与被判断的文本同语言，判断更稳。所以中国
    -- 团队读英文技术文档时，界面要中文而这一列该是 'en'，一个开关按不下去这两件事。
    --
    -- 取值收在 CHECK 里而不是应用层：这一列会被用来挑一张编译期常量表，写进一个
    -- 没有对应表的值只会静默回落到英文，不报错——那种错最难查
    ontology_lang TEXT NOT NULL DEFAULT 'en',
    -- 默认库永远 open（规则在 API 强制，这里是 DB 级双保险）
    CONSTRAINT kb_default_open CHECK (NOT is_default OR visibility = 'open'),
    CONSTRAINT knowledge_bases_ontology_lang_chk CHECK (ontology_lang IN ('en', 'zh'))
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
