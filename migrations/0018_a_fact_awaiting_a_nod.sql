-- 等人点头的事实（见 docs/decisions/0015）。
--
-- 起因是一次实测：对话里说「记住 Acme 把总部搬到了深圳」，助手回「已记录」，
-- 而图里落下的是一条 **空谓词、0.9 置信** 的边——本体里没有「搬迁到」这类关系，
-- 抽取落不上就留空（按 0010 这是对的）。**说的和进去的不是一回事，人无从发现。**
--
-- **自己一张表，不进 `facts`。**
--
-- 第一版是给 `facts` 加一列 `nod`。那正是 0013 记下的那个错的形状：
--
-- > 试过塞进 `facts` 加一位 `derived_by_rule` 标记，那一版的问题是**失败方向反了**：
-- > 仓库里有四十多处读 `facts` 的查询，其中只有一处认识那个标记……
-- > 分开之后忘了 UNION 的后果是**看不见**派生，而不是**混进去**。
--
-- 今天有 27 处查询按 `invalidated_at IS NULL` 捞活事实，分布在 6 个文件。
-- 逐个补过滤，漏一处就有一条没人点头的事实混进图里——而这张表存在的全部理由
-- 就是防这件事。分开之后，忘了读它的后果是「待确认队列看不见」，不是「未确认
-- 的进了图」。
--
-- **只拦交互式的单条写入，不拦批量摄入。** 灌 500 篇文档抽出一万条事实，
-- 让人逐条确认是不可能的；那条路仍旧乐观写入 + 事后审阅。而 `remember`
-- 一次一句、人就在对话里，确认成本最低的那一刻恰好就在眼前。
CREATE TABLE pending_facts (
    id           UUID PRIMARY KEY,
    kb_id        UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    subject_id   UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    -- 可空，与 `facts.predicate_id` 一致：本体里没有对应关系时留空（0010）。
    -- **而人要看见的正是这个空** —— 那条实测里空谓词就是该被拒的理由
    predicate_id UUID REFERENCES relation_types(id) ON DELETE SET NULL,
    object_id    UUID REFERENCES entities(id) ON DELETE CASCADE,
    object_value JSONB,
    -- 模型原话里的说法。谓词落空时，人靠它判断「本体该不该长出这条关系」
    proposed_predicate TEXT,

    valid_from   TIMESTAMPTZ,
    valid_from_precision TEXT,
    valid_to     TIMESTAMPTZ,
    valid_to_precision   TEXT,
    confidence   REAL NOT NULL DEFAULT 0.5,

    -- 出自哪一句记忆。**确认界面要把原句和三元组并排显示**——
    -- 只列三元组等于要人凭空判断它对不对
    chunk_id     UUID NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    -- 谁的话。`remember` 今天不记这个，一并补上（0015 点名的缺口）
    proposed_by  UUID REFERENCES users(id),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 待确认队列按库捞，还要数个数
CREATE INDEX pending_facts_kb_idx ON pending_facts (kb_id, created_at DESC);
-- 一句记忆抽出的若干条要一起显示
CREATE INDEX pending_facts_chunk_idx ON pending_facts (chunk_id);

-- 拒绝过的不该被下一轮重抽刷回来。
--
-- `concept_mappings` 那边靠 `status = 'rejected'` 挡住重复提议；这里同理，
-- 但**拒绝的记录不能留在 pending_facts 里**——那张表的语义是「等着人看」，
-- 混进已经看过的会让计数说谎。所以另开一张，只记「这个三元组在这个库里
-- 被拒过」。
CREATE TABLE rejected_facts (
    kb_id        UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    subject_id   UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    -- 谓词可空，所以进不了主键；用 COALESCE 过的表达式索引来查重
    predicate_id UUID REFERENCES relation_types(id) ON DELETE SET NULL,
    object_id    UUID REFERENCES entities(id) ON DELETE CASCADE,
    rejected_by  UUID REFERENCES users(id),
    rejected_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX rejected_facts_lookup_idx
    ON rejected_facts (kb_id, subject_id, object_id);
