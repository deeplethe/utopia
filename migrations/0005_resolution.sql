-- 实体消解：同名≠同人。
-- 设计见 docs/DESIGN.md §4：名字只是候选召回线索，身份由上下文（画像向量 + 关系兼容性）决定；
-- 宁分勿合，灰区先落审核队列，LLM 攒批裁决在后台跑，人工终审兜底。



-- 消解审核队列：疑似同一实体的灰区对。
-- stage: adjudicating = 等 LLM 攒批裁决；human = LLM 不确定/未配模型，等人工终审。
-- status: pending → merged / kept。
CREATE TABLE resolution_reviews (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    left_id    UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    right_id   UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    score      REAL NOT NULL DEFAULT 0,
    reason     TEXT,
    stage      TEXT NOT NULL DEFAULT 'adjudicating' CHECK (stage IN ('adjudicating', 'human')),
    status     TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'merged', 'kept')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at TIMESTAMPTZ,
    decided_by UUID REFERENCES users(id)
);
CREATE UNIQUE INDEX resolution_reviews_pair_idx
    ON resolution_reviews (kb_id, least(left_id, right_id), greatest(left_id, right_id))
    WHERE status = 'pending';
CREATE INDEX resolution_reviews_kb_pending_idx
    ON resolution_reviews (kb_id, created_at) WHERE status = 'pending';

-- LLM 裁决缓存：同一对（名字 + 上下文摘要哈希）不重复付费；same 为 NULL 表示模型也不确定
CREATE TABLE resolution_verdicts (
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    pair_key   TEXT NOT NULL,
    same       BOOLEAN,
    confidence REAL NOT NULL DEFAULT 0,
    model      TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kb_id, pair_key)
);

-- 合并日志：记录被移动/作废的事实与目标实体画像快照，支持精确回滚
CREATE TABLE entity_merges (
    id                    UUID PRIMARY KEY,
    kb_id                 UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    source_id             UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    target_id             UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    moved_subject_facts   UUID[] NOT NULL DEFAULT '{}',
    moved_object_facts    UUID[] NOT NULL DEFAULT '{}',
    invalidated_facts     UUID[] NOT NULL DEFAULT '{}',
    -- 合并后时态对账产生的修正行（成因是合并本身，回滚时随之撤销）
    temporal_corrections  UUID[] NOT NULL DEFAULT '{}',
    target_profile_before vector,
    target_profile_n_before INTEGER NOT NULL DEFAULT 0,
    -- 类型调和（concept 目标被具体类型升格）的回滚快照
    target_type_before    UUID REFERENCES entity_types(id),
    -- NULL = 自动合并（LLM 裁决高置信）
    merged_by             UUID REFERENCES users(id),
    reason                TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    reverted_at           TIMESTAMPTZ
);
CREATE INDEX entity_merges_kb_idx ON entity_merges (kb_id, created_at DESC);
