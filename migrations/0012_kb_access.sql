-- 0012: KB 级访问控制 + 部署配置。
-- 设计（2026-08-26 拍板）：部署角色挂隐形 workspace（memberships 不动）；
-- 每个 KB 自带角色矩阵（kb_members，viewer|editor|admin），在库自己的 Settings 里配置；
-- open 库无矩阵记录时按部署角色行事，restricted 库仅矩阵成员可见（外人 NotFound）。
-- 注册开关从 env 升级为库内配置（/admin 可切）。

ALTER TABLE knowledge_bases ADD COLUMN visibility TEXT NOT NULL DEFAULT 'open'
    CHECK (visibility IN ('open', 'restricted'));
-- 默认库永远 open（is_default 列见 0001；规则在 API 强制,这里 DB 级双保险）
ALTER TABLE knowledge_bases ADD CONSTRAINT kb_default_open
    CHECK (NOT is_default OR visibility = 'open');

CREATE TABLE kb_members (
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role       TEXT NOT NULL CHECK (role IN ('viewer', 'editor', 'admin')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kb_id, user_id)
);

CREATE TABLE deployment_settings (
    singleton         BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    open_registration BOOLEAN NOT NULL DEFAULT TRUE,
    -- 任务 worker 并发数（系统设置可改；调度循环热读，改动即时生效）
    worker_concurrency INT NOT NULL DEFAULT 4
        CHECK (worker_concurrency BETWEEN 1 AND 32),
    -- 本体铺进抽取提示词的字符预算，超了就改成按分块检索候选。
    -- 放部署设置而不是环境变量：这一档要能不重启就改——定它需要每个本体规模
    -- 下全量内联与按块检索各一组对照，靠重启服务改一档的话，那条曲线不会有人
    -- 跑第二遍。24000 字符（约 6000 token）是拍的，正等那条曲线来定
    ontology_prompt_budget INTEGER NOT NULL DEFAULT 24000
);
INSERT INTO deployment_settings DEFAULT VALUES;
