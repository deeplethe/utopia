-- 一条事实在某一时刻属于哪个实体（0019 第二刀 / #336）。
--
-- 合并是**原地改写**：`UPDATE facts SET subject_id = target WHERE subject_id = source`。
-- 行上因此只剩合并之后的样子，而被改写的行 id 一条不落地记在
-- `entity_merges.moved_subject_facts` / `moved_object_facts` 里——「当时属于谁」
-- 查得回来，只是查不出一个列，得走这张表。
--
-- **放成 SQL 函数而不是每个读点各拼一段 CTE**，与 `fact_surface_predicate` 同一个理由：
-- 这类映射一旦散开，漏掉一处就是一张安静地把事实挂错人的图（0019 的风险那一节）。
--
-- `at IS NULL` 直接回当前值：现在这条路一次表都不查，回放才付代价。
CREATE FUNCTION fact_owner_at(
    fact_id       UUID,
    current_owner UUID,
    at            TIMESTAMPTZ,
    -- false = 主语侧，true = 宾语侧
    on_object     BOOLEAN
) RETURNS UUID
LANGUAGE plpgsql
STABLE
AS $$
DECLARE
    owner UUID := current_owner;
    undone UUID;
    applied UUID;
BEGIN
    IF at IS NULL OR current_owner IS NULL THEN
        RETURN current_owner;
    END IF;

    -- ① 撤销：T 之后才发生、且**现在仍生效**的合并。只有它们的移动还留在当前行上。
    --    已撤销的合并不在此列——`revert_merge` 早把那些行搬回去了，再撤一次
    --    会把事实挂到一个它从来没待过的实体上。
    --    链式（A→B 再 B→C）取**最早**的那一次：它的 source 就是 T 时刻的持有者。
    SELECT m.source_id INTO undone
      FROM entity_merges m
     WHERE m.reverted_at IS NULL
       AND m.created_at > at
       AND fact_id = ANY(
             CASE WHEN on_object THEN m.moved_object_facts ELSE m.moved_subject_facts END)
     ORDER BY m.created_at
     LIMIT 1;
    IF undone IS NOT NULL THEN
        owner := undone;
    END IF;

    -- ② 应用：T 当时正生效、后来被撤销的合并。它的移动已经被撤掉，当前行里看不见，
    --    但在 T 那一刻，这条事实确实挂在 target 上——记录轴上那段窗口是真实存在过的。
    SELECT m.target_id INTO applied
      FROM entity_merges m
     WHERE m.reverted_at IS NOT NULL
       AND m.created_at <= at
       AND m.reverted_at > at
       AND m.source_id = owner
       AND fact_id = ANY(
             CASE WHEN on_object THEN m.moved_object_facts ELSE m.moved_subject_facts END)
     ORDER BY m.created_at DESC
     LIMIT 1;
    IF applied IS NOT NULL THEN
        owner := applied;
    END IF;

    RETURN owner;
END;
$$;

-- 撤销那一步按 (reverted_at, created_at) 找，且要按数组成员判断——数组含判断用不上
-- B-tree，所以先用这个部分索引把行集收窄到「仍生效的合并」，剩下的行数在一个库里
-- 是个位数到几十的量级。
CREATE INDEX entity_merges_live_idx
    ON entity_merges (kb_id, created_at)
    WHERE reverted_at IS NULL;
