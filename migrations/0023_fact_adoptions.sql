-- 采纳表层谓词时，哪条事实被改写成了哪条。
--
-- 没有这张表时改写只留下 facts.supersedes 一个指针，而它覆盖不了"并入已存在
-- 事实"那种情形：旧行被作废、没有任何后继指向它。后果有两个——撤销时找不回
-- 它去了哪，以及实体历史把"已作废且无后继"判成 rejected，于是界面会说
-- "这条记录被撤回了"，而它其实原封不动地并进了另一条断言。
--
-- 顺带补上治理要求的那一半：审计行此前只记了"改写 49 条"这个总数，
-- 答不出"具体哪 49 条"。
CREATE TABLE fact_adoptions (
    -- 一次采纳动作的批次；撤销以它为单位
    batch_id     UUID NOT NULL,
    kb_id        UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    predicate_id UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    old_fact_id  UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    new_fact_id  UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    -- superseded = 新写一行取代旧行；merged = 并入已存在的行
    mode         TEXT NOT NULL,
    -- 撤销不删行：抹掉"发生过什么"与账本的规矩相反，而且撤销本身也是
    -- 一次人的决定，实体历史要据此归因
    reverted_at  TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (batch_id, old_fact_id)
);

-- 实体历史逐条问"这条是不是被并走了"，走这个索引
CREATE INDEX fact_adoptions_old_idx ON fact_adoptions (old_fact_id);
-- 按库列出可撤销的批次
CREATE INDEX fact_adoptions_kb_idx ON fact_adoptions (kb_id, created_at DESC);
