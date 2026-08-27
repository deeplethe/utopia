-- 问数：数据源两层模型。
-- 系统层注册连接（凭据集中、跨 KB 复用），知识库层挂载授权（问数权限跟 KB 走）。

CREATE TABLE data_sources (
    id           UUID PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    -- 首发仅 postgres；mysql/clickhouse 后续加驱动
    engine       TEXT NOT NULL CHECK (engine IN ('postgres')),
    -- 连接串（含凭据）。与 llm_settings 的 api key 同待遇：静态加密尚未实现，见 README 的 Status 段
    conn_string  TEXT NOT NULL,
    created_by   UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_test_at TIMESTAMPTZ,
    last_test_ok BOOLEAN
);

-- KB 挂载：本库的 Chat 才能问到挂载的源
CREATE TABLE kb_data_sources (
    kb_id          UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    data_source_id UUID NOT NULL REFERENCES data_sources(id) ON DELETE CASCADE,
    mounted_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kb_id, data_source_id)
);
