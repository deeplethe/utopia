-- KB 级访问控制 + 部署配置。
-- 部署角色挂隐形 workspace（memberships 不动）；每个 KB 自带角色矩阵，
-- 在库自己的 Settings 里配置。注册开关是库内配置而不是 env（/admin 可切）。

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
