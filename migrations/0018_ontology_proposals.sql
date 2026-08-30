-- 本体提案落库。
--
-- 从前它只活在浏览器内存里（`Ontology.tsx` 的 `useState<OntologyProposals>`）：
-- 刷新一次、切走一次、崩一次，整批提议就没了，想再看只能重跑一次模型。
--
-- **丢的不是原材料。** 未匹配的说法一直存在 `ontology_misses` 里。丢的是**聚类
-- 结果**——哪些说法被归到同一条提议底下、以及"采纳后将重新归类 N 条"那个估算。
-- 而那正是唯一能查证过并的东西：0003 记着模型建议把 `optimized_for` 并进
-- `runs_on`（"为 RTX 优化"不等于"跑在 RTX 上"），**它是靠 tooltip 里看得见归并了
-- 哪些说法才被抓出来的**，并且成了"不该全自动归并"的直接证据。能查证的东西不该
-- 只活在一个页面的生命周期里。
CREATE TABLE ontology_proposals (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- 提案分档，与接口返回的四个小节同名：
    -- entity_types | relation_types | attribute_types | map_to
    section    TEXT NOT NULL,
    key        TEXT NOT NULL,
    -- 那一条提案的原样：label、description、reason、forms、datatype、temporal…
    --
    -- **存 JSONB 而不是拆成列**：四个小节的形状本来就不同（关系有 temporal 与
    -- forms，属性有 datatype，map_to 有目标），拆开要么四张表要么一张稀疏宽表。
    -- 前端消费的也正是这个 JSON，存原样等于不改契约。要查归并了哪些说法仍然
    -- 查得动（`payload->'forms'`）
    payload    JSONB NOT NULL,
    -- open = 还等着人看；adopted / rejected = 已经有人表过态
    status     TEXT NOT NULL DEFAULT 'open'
               CHECK (status IN ('open', 'adopted', 'rejected')),
    decided_by UUID REFERENCES users(id),
    decided_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 同一个库里同一档下的同一个 key 只有一条。重跑 Suggest 是刷新它，不是再堆一条
    UNIQUE (kb_id, section, key)
);

-- "还有多少条等着看"是这张表最常被问的问题（0003 的另一个缺口：关掉自动扩展
-- 开关之后没有"自上次以来有 N 个新说法"的提醒，信号在面板里但没人主动看）
CREATE INDEX ontology_proposals_open_idx
    ON ontology_proposals (kb_id, created_at DESC)
    WHERE status = 'open';
