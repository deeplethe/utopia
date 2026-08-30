-- 核心：多租户表、任务队列、访问控制与部署配置。
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

-- KB 级访问控制。部署角色挂隐形 workspace（memberships 不动）；每个 KB 自带
-- 角色矩阵，在库自己的 Settings 里配置
CREATE TABLE kb_members (
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role       TEXT NOT NULL CHECK (role IN ('viewer', 'editor', 'admin')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 谁把这个成员加进来的（无人可归时留 NULL，展示退化为只有时间）
    added_by   uuid REFERENCES users(id) ON DELETE SET NULL,
    PRIMARY KEY (kb_id, user_id)
);

CREATE TABLE deployment_settings (
    singleton         BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    open_registration BOOLEAN NOT NULL DEFAULT TRUE,
    -- 任务 worker 并发数（系统设置可改；调度循环热读，改动即时生效）。
    -- **这是外层兜底而不是节流**：真正的节流交给按模型的信号量（见
    -- model_concurrency），这里只防任务无限堆积。要明显大于各模型限额之和，
    -- 否则被限流的任务会占满槽位把别的饿死
    worker_concurrency INT NOT NULL DEFAULT 32
        CHECK (worker_concurrency BETWEEN 1 AND 32),
    -- 本体铺进抽取提示词的字符预算，超了就改成按分块检索候选。
    -- 放部署设置而不是环境变量：这一档要能不重启就改——定它需要每个本体规模
    -- 下全量内联与按块检索各一组对照，靠重启服务改一档的话，那条曲线不会有人
    -- 跑第二遍。24000 字符（约 6000 token）是拍的，正等那条曲线来定
    ontology_prompt_budget INTEGER NOT NULL DEFAULT 24000,
    -- 没在 model_concurrency 里配过的模型走这个缺省
    default_model_concurrency INT NOT NULL DEFAULT 10,
    -- JWT 签名密钥。**首次启动自动生成**，于是「照 README 跑起来」和「安全」
    -- 不再是两件要分别做的事——默认值 dev-secret-change-me 上生产这类事故，
    -- 靠提醒是防不住的。UTOPIA_JWT_SECRET 仍然优先于本列：轮换密钥、或者要
    -- 多个实例显式对齐时填环境变量即可，那条路没有被关掉
    jwt_secret TEXT,
    -- 新库的 ontology_lang 缺省，含义见 knowledge_bases.ontology_lang
    default_ontology_lang TEXT NOT NULL DEFAULT 'en',
    CONSTRAINT deployment_default_ontology_lang_chk
        CHECK (default_ontology_lang IN ('en', 'zh'))
);
INSERT INTO deployment_settings DEFAULT VALUES;

-- 并发限制**按模型算，不按部署算**。真正的约束是模型供应商的速率限制，那是
-- 按模型（连同 base_url）来的：本地 Ollama 可能只扛 2 个并发，托管 API 能吃
-- 50——一个全局数字管两者本来就不对。
--
-- 限流放在 LLM 调用处而不是任务调度处：不调模型的任务（文件夹同步）不该受它
-- 约束，调不同模型的任务（抽取用 chat、摄入用 embedding）之间也不该互相挤。
CREATE TABLE model_concurrency (
    base_url        TEXT NOT NULL,
    model           TEXT NOT NULL,
    max_concurrent  INT  NOT NULL CHECK (max_concurrent BETWEEN 1 AND 256),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (base_url, model)
);
