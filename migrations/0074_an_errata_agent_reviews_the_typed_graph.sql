-- 勘误 agent（0044 决定 7，第六刀）：抽取之后，一个 agent 按文档复审类型化图谱。
-- 结构先报的（主语或宾语在属性声明的类之外、文档里找不到的名字、日期属性没有日期）
-- 先看，其余抽样；每一次撤、改、加记成一笔动作，带着文档的原话当证据；会牵动图外
-- 东西的动作留给人（0027 那道闸门），agent 不动手。
--
-- `errata_runs`：一份文档一次复审的账——看了几条、问了几次、花了多少 token。度量
-- 「精度换来多少、撤错多少、花了多少」按次数算，不按行数。
--
-- `errata_actions`：每一条看过的事实一行（keep 也记：「没看过的」就是没行的，复审不重复），
-- 加的事实一行。`statement_id` + `predicate_id` 是撤销站得住的关键：物化下一轮会把
-- 活着的陈述再算成同一条类型化行，除非它知道这条（陈述, 属性）被勘误撤过。别的文档
-- 说了同一件事照样算——勘误看的是这份文档，撤的是这份文档产生的那条。
--
-- status：applied 落了地；held 闸门留给人；refused agent 的说法没过验证（引文不在
-- 文档里、名字不在库里、属性不存在），记下来不执行；rejected 人否了留给人的那笔。

CREATE TABLE errata_runs (
    id                UUID PRIMARY KEY,
    kb_id             UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    document_id       UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- 这一次送去看的：结构报出来的几条、抽样的几条
    flagged           INT NOT NULL DEFAULT 0,
    sampled           INT NOT NULL DEFAULT 0,
    requests          INT NOT NULL DEFAULT 0,
    -- 端点报的用量；端点不报就空着，不编数字
    prompt_tokens     BIGINT,
    completion_tokens BIGINT,
    started_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at       TIMESTAMPTZ
);
CREATE INDEX errata_runs_document_idx ON errata_runs (document_id, started_at DESC);

CREATE TABLE errata_actions (
    id           UUID PRIMARY KEY,
    kb_id        UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    run_id       UUID NOT NULL REFERENCES errata_runs(id) ON DELETE CASCADE,
    document_id  UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- 看的那条类型化事实；add 没有
    fact_id      UUID REFERENCES facts(id) ON DELETE CASCADE,
    -- 这份文档里产生它的那条陈述与它的属性：物化按这一对跳过
    statement_id UUID REFERENCES facts(id) ON DELETE CASCADE,
    predicate_id UUID REFERENCES relation_types(id) ON DELETE CASCADE,
    -- 为什么送去看：结构报的哪一条；空 = 抽样
    flag         TEXT CHECK (flag IN ('domain', 'range', 'name_absent', 'no_date')),
    action       TEXT NOT NULL CHECK (action IN ('keep', 'retract', 'revise', 'add')),
    reason       TEXT NOT NULL DEFAULT '',
    -- 文档的原话：撤、改、加都得引一句；keep 不用
    quote        TEXT,
    -- 动作指向的那条事实（改成什么、加什么；撤的就是看的那条），名字与 id 都在，
    -- 队列卡片读名字，执行读 id
    proposed     JSONB,
    new_fact_id  UUID REFERENCES facts(id) ON DELETE SET NULL,
    status       TEXT NOT NULL CHECK (status IN ('applied', 'held', 'refused', 'rejected')),
    -- 闸门的理由（0027 的写法：`derived 2` / `answered 1` / `contradiction CEO of`）或拒绝的理由
    detail       TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at   TIMESTAMPTZ,
    decided_by   UUID REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX errata_actions_kb_status_idx ON errata_actions (kb_id, status, created_at);
CREATE INDEX errata_actions_fact_idx ON errata_actions (fact_id) WHERE fact_id IS NOT NULL;
CREATE INDEX errata_actions_statement_idx ON errata_actions (statement_id, predicate_id)
    WHERE status = 'applied' AND action IN ('retract', 'revise');
