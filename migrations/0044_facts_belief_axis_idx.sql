-- 认知轴一直没有索引。
--
-- 0004 给 facts 建了三个索引，全在世界轴上（valid_from/valid_to），且都带
-- `WHERE invalidated_at IS NULL`——只认活行。这套索引服务的是"某时刻什么为真"。
--
-- 但账本还有另一根轴：**什么时候写进来的、什么时候被推翻的**。按这根轴开窗
--（"上个季度我们的认知有哪些变化"）以前没有查询路径，所以缺索引一直没露头。
-- changes 工具把这根轴暴露给了 agent，缺索引就成了每问一次全表扫一遍。
--
-- 注意作废那条是**部分索引且条件取反**：活行的 invalidated_at 全是 NULL，
-- 把它们收进来只是让索引跟表一样大。被推翻的事实天然是少数。
CREATE INDEX IF NOT EXISTS facts_recorded_idx
    ON facts (kb_id, recorded_at DESC);

CREATE INDEX IF NOT EXISTS facts_invalidated_idx
    ON facts (kb_id, invalidated_at DESC)
    WHERE invalidated_at IS NOT NULL;
