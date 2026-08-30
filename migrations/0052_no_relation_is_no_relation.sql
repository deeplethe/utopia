-- 「说不出是什么关系」不该是一种关系。
--
-- 与 0009 对 `concept` 做的事同构。`related_to` 编码的是**抽取器抽到了一条边，
-- 但本体里没有对应的关系**——那是控制流，不是词汇。它却以 `builtin` 关系的身份
-- 躺在 `relation_types` 里，在本体页上跟真关系并排列着。
--
-- 证据在删之前量过：某个 348 块的库里 533 条事实挂在 `related_to` 上，
-- **533 条全部带着原文说法**（`fact_evidence.proposed_predicate`），一条不落。
-- 所以删掉它一个字的信息都不丢——原意一直存在证据里，被那行假词汇盖着而已。
--
-- 而且删掉之后**信息更多**：今天这 533 条边统统显示"有关联"，
-- 之后各自显示原文说的 `acquired` / `runs_on` / `sued`。同一份数据，读者看得见的
-- 从一个词变成五百多个词。
--
-- 一条事实带多种说法的只占 3.0%（16/533），所以"显示哪一个"不是拦路虎：
-- 取出现最多的那个，与 `predicate_match::merge_key` 挑规范 key 同一个规矩。
--
-- **有 420 条例外，全是历史遗留。** 全库 5934 条兜底事实里，420 条拿不出原文说法，
-- 而它们**全部**来自 `Industry Corpus`(275) 与 `General`(145) 两个最早的库——
-- 那时 `add_evidence` 还没无条件记录 `proposed_predicate`。8 月 30 日之后建的库
-- 一条都没有。这类事实改完会显示成空，但它们本来显示的是"有关联"，
-- 信息量同样是零，没有变糟。
--
-- **数据直接改，不做搬迁**：现在库里全是测试数据，产品未发布。

ALTER TABLE facts ALTER COLUMN predicate_id DROP NOT NULL;

UPDATE facts SET predicate_id = NULL
 WHERE predicate_id IN (SELECT id FROM relation_types WHERE key = 'related_to');

DELETE FROM relation_types WHERE key = 'related_to';

-- 没有谓词的事实**显示什么**。
--
-- 做成函数而不是在每条读查询里塞一段子查询：读事实的路径有六条以上
--（图的边、实体面板、变更历史、低置信审核、文档产出、消解画像），
-- 六份同样的 SQL 迟早分叉，而分叉在这里的后果是同一条边在不同页面上叫不同的名字。
--
-- **确定性**：出现次数相同时按字典序，所以同一条事实每次显示同一个词。
CREATE FUNCTION fact_surface_predicate(fact uuid) RETURNS text
LANGUAGE sql STABLE AS $$
    SELECT e.proposed_predicate
      FROM fact_evidence e
     WHERE e.fact_id = fact AND e.proposed_predicate IS NOT NULL
     GROUP BY e.proposed_predicate
     ORDER BY count(*) DESC, e.proposed_predicate
     LIMIT 1
$$;
