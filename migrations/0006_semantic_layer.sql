-- 语义层：问数的数据源，与「业务概念 → 数据资产」的口径映射。
--
-- 数据源两层模型：
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

-- 语义层的「概念 → 数据资产」映射搬出本体，见 docs/decisions/0011。
--
-- 从前它是一条 `mapped_to` 事实：主语是概念实体，宾语是塞在 `object_value`
-- 里的一份 JSON 配置，而 `mapped_to` 本身是 `relation_types` 里的一行，
-- 与 `works_at` 并列。
--
-- 拆开的理由，三条都不是洁癖：
--
-- 1. **它不是关于世界的断言。** 本体回答「世界上有什么」，这个回答的是
--    「这个数在我们的数据库里怎么算」。与 0009 说「concept 是控制流不是词表」、
--    0010 说「related_to 是兜底不是关系」是同一句话的第三次应用。
--
-- 2. **「确认」这个动作已经在违反账本的地基。** `confirm_fact` 是
--    `UPDATE facts SET confidence = 1.0`——原地改。而账本是 append-only 的，
--    纠正事实要插新行 + supersedes，因为认知变更本身是信息（0001 P0）。
--    确认口径改的不是认知，是「这条配置生效了没有」。一个需要原地改状态的
--    东西住在一张不许原地改的表里，本身就是它不合身的证据。
--
-- 3. **形状对不上。** 真正的字段是 source / table / expr / sql / unit /
--    summary，全塞在一个 JSONB 里：查不动（「哪些概念映射到了 orders」要扒
--    JSON）、约束不了（唯一性粒度 (概念,源) 藏在 object_value 内部，
--    数据库管不到，今天靠流程而不是约束）。

CREATE TABLE concept_mappings (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- 被映射的业务概念（Metric / Dimension 那类实体）
    concept_id UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    -- 挂载的数据源。**它进主键**：同一个概念在不同源上有不同定义是有意支持的，
    -- 而同一个概念在同一个源上只该有一条——这条从前靠确认流程显式闭合，
    -- 现在由数据库管
    source     TEXT NOT NULL,
    -- 怎么算。字段展开成列而不是继续塞 JSON——它们是这张表存在的理由
    table_name TEXT,
    expr       TEXT,
    sql        TEXT,
    unit       TEXT,
    summary    TEXT,
    -- 派生指标（比如「转化率 = 成交数 / 访问数」）：算出来的，不是表里的列
    derived    BOOLEAN NOT NULL DEFAULT FALSE,

    -- **状态而不是置信度。** 从前借事实的 confidence 表达「提议 0.6 / 确认 1.0」，
    -- 那是把一个二值状态编码成浮点数，还顺带让它落进「低置信事实」那一档。
    -- 这里说清楚它是什么
    status     TEXT NOT NULL DEFAULT 'proposed'
               CHECK (status IN ('proposed', 'confirmed', 'rejected')),
    -- NULL = 还没人表态。确认与拒绝都留痕：拒绝过的不该被下一轮探索刷回待看。
    --
    -- 裸外键，与 `entity_merges.merged_by`、`ontology_proposals.decided_by`
    -- 那几处一致——**用户是软删除的，生产代码里没有 `DELETE FROM users`**，
    -- 所以这条外键的删除规则永远不会被触发，而归因保住了。
    decided_by UUID REFERENCES users(id),
    decided_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (kb_id, concept_id, source)
);

-- 问数只读确认过的（chat.rs 注入 system prompt），而 Review 只捞待表态的。
-- 两条查询都按 kb + status 走
CREATE INDEX concept_mappings_status_idx ON concept_mappings (kb_id, status);

-- 口径演变留痕。**不做双时态**：口径没有「有效时间」与「记录时间」两条轴，
-- 它只有「什么时候生效的」一条。硬套账本那套是把复杂度搬过来，不是解决它。
CREATE TABLE concept_mapping_revisions (
    id         UUID PRIMARY KEY,
    mapping_id UUID NOT NULL REFERENCES concept_mappings(id) ON DELETE CASCADE,
    -- 改之前那一版的全文。存快照而不是差异：读的时候要的是「当时是什么」，
    -- 而差异要从头重放才能回答这个问题
    before     JSONB NOT NULL,
    changed_by UUID REFERENCES users(id),
    changed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX concept_mapping_revisions_idx
    ON concept_mapping_revisions (mapping_id, changed_at DESC);

-- 旧的 mapped_to 事实不迁移：仓库尚未发布，库里都是 mock 知识（与 #125 同）。
-- 它们留在账本里不碍事——`confirmed_mappings` 那条查询会随代码一起改掉，
-- 于是再没有人读它们。
--
-- **真发布之后就没有这个便利了**，写在这里免得下次照抄。
