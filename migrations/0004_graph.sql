-- 0004: 图谱层 —— 轻量本体 + 实体 + 双时态事实账本 + 证据链
-- 设计见 docs/DESIGN.md §3：事实 append-only；抽取错误设 invalidated_at，事实变化闭合 valid_to。

CREATE TABLE entity_types (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    label      TEXT NOT NULL,
    color      TEXT NOT NULL DEFAULT '#64748b',
    builtin    BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (kb_id, key)
);

CREATE TABLE relation_types (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    label      TEXT NOT NULL,
    -- 时间语义：state 状态型(区间) / event 事件型(时点) / eternal 永恒型(无时间)
    temporal   TEXT NOT NULL DEFAULT 'state' CHECK (temporal IN ('state', 'event', 'eternal')),
    -- 基数唯一（同一时刻单值），时态冲突检测（自动闭合 valid_to）的依据：
    -- functional = 主语侧唯一；inverse_functional = 宾语侧唯一（一个项目一个 leader）
    functional BOOLEAN NOT NULL DEFAULT FALSE,
    inverse_functional BOOLEAN NOT NULL DEFAULT FALSE,
    builtin    BOOLEAN NOT NULL DEFAULT FALSE,
    -- 属性系统：属性 = 值域为字面量的关系（RDF datatype property），一表两用。
    -- 属性值走 facts.object_value 通道，时态/证据/Review 全套复用
    kind       TEXT NOT NULL DEFAULT 'relation' CHECK (kind IN ('relation', 'attribute')),
    -- 属性挂在哪个类下（attribute 必填；类删则属性随删）
    domain_type_id UUID REFERENCES entity_types(id) ON DELETE CASCADE,
    datatype   TEXT CHECK (datatype IN ('text', 'number', 'date', 'bool')),
    unit       TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (kb_id, key)
);

CREATE TABLE entities (
    id             UUID PRIMARY KEY,
    kb_id          UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    type_id        UUID NOT NULL REFERENCES entity_types(id) ON DELETE RESTRICT,
    canonical_name TEXT NOT NULL,
    aliases        TEXT[] NOT NULL DEFAULT '{}',
    attrs          JSONB NOT NULL DEFAULT '{}',
    -- 被合并后指向存活实体（合并可回滚，P2 后续）
    merged_into    UUID REFERENCES entities(id),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX entities_kb_type_name_idx
    ON entities (kb_id, type_id, lower(canonical_name)) WHERE merged_into IS NULL;
CREATE INDEX entities_kb_idx ON entities (kb_id);
-- 跨类型同名召回（类型漂移处理）：上面的唯一索引带 type_id 前缀，跨类型查询用不上
CREATE INDEX entities_kb_name_idx
    ON entities (kb_id, lower(canonical_name)) WHERE merged_into IS NULL;

-- 事实账本：SPO + 双时间轴（append-only，永不 DELETE）
CREATE TABLE facts (
    id              UUID PRIMARY KEY,
    kb_id           UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    subject_id      UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    predicate_id    UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    object_id       UUID REFERENCES entities(id) ON DELETE CASCADE,
    object_value    JSONB,
    valid_from      TIMESTAMPTZ,
    valid_to        TIMESTAMPTZ,
    valid_precision TEXT NOT NULL DEFAULT 'day',
    recorded_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    invalidated_at  TIMESTAMPTZ,
    confidence      REAL NOT NULL DEFAULT 1.0,
    derived_by_rule UUID,
    supersedes      UUID REFERENCES facts(id),
    CHECK (object_id IS NOT NULL OR object_value IS NOT NULL)
);
-- 热路径部分索引：作废行不进索引（账本边界与清理，DESIGN.md §3.1）
CREATE INDEX facts_live_subject_idx ON facts (kb_id, subject_id) WHERE invalidated_at IS NULL;
CREATE INDEX facts_live_object_idx  ON facts (kb_id, object_id)  WHERE invalidated_at IS NULL;
CREATE INDEX facts_live_time_idx    ON facts (kb_id, valid_from, valid_to) WHERE invalidated_at IS NULL;
-- 时态冲突检测的不变量点查：主语侧与宾语侧各一（开放期 + 未作废）
CREATE INDEX facts_open_pair_idx ON facts (kb_id, subject_id, predicate_id)
    WHERE valid_to IS NULL AND invalidated_at IS NULL;
CREATE INDEX facts_open_obj_pair_idx ON facts (kb_id, object_id, predicate_id)
    WHERE valid_to IS NULL AND invalidated_at IS NULL;
-- 认知轴。上面那几个索引全在世界轴上、且都只认活行，服务的是"某时刻什么为真"；
-- 这两个服务的是另一个问题——**什么时候写进来的、什么时候被推翻的**
--（"上个季度我们的认知有哪些变化"，见 chat 的 changes 工具）。
-- 作废那条是部分索引且条件取反：活行的 invalidated_at 全是 NULL，
-- 把它们收进来只会让索引跟表一样大，而被推翻的事实天然是少数
CREATE INDEX facts_recorded_idx ON facts (kb_id, recorded_at DESC);
CREATE INDEX facts_invalidated_idx ON facts (kb_id, invalidated_at DESC)
    WHERE invalidated_at IS NOT NULL;

-- 证据链：事实 ↔ 原文分块（溯源一等公民）
CREATE TABLE fact_evidence (
    fact_id  UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    chunk_id UUID NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    quote    TEXT,
    -- 证据出处版本：出自哪份文档的第几版（版本对账与"证据过期"展示的判定依据）
    document_id UUID REFERENCES documents(id) ON DELETE CASCADE,
    doc_version INT,
    PRIMARY KEY (fact_id, chunk_id)
);

-- 文档的图谱抽取状态（与摄入管道状态分离：两段式可用）
ALTER TABLE documents ADD COLUMN graph_status TEXT NOT NULL DEFAULT 'none'
    CHECK (graph_status IN ('none', 'queued', 'extracting', 'done', 'failed'));

-- 时态冲突（S3）：自动闭合拿不准的进审，人裁 close / keep / reject_new
CREATE TABLE fact_conflicts (
    id          UUID PRIMARY KEY,
    kb_id       UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    old_fact_id UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    new_fact_id UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    -- no_time | simultaneous | low_confidence
    reason      TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved')),
    -- closed | kept_both | rejected_new
    resolution  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    UNIQUE (old_fact_id, new_fact_id)
);
CREATE INDEX fact_conflicts_open_idx ON fact_conflicts (kb_id) WHERE status = 'open';
