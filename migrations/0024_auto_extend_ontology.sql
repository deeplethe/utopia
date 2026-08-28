-- 本体自动扩展的开关，以及"人说过不要"的记忆。
--
-- 开关：冷启动自动扩本体此前靠"本体有没有被碰过"来判断该不该跑。那个判据
-- 是从行为里推断意图，推错了会很荒唐——在提案上点一次 Add 就会永久关掉
-- 建议功能，因为那记了一条带操作人的本体动作。而且它一旦为假就永不再真，
-- 本体被冻结在第一批文档碰巧包含的词汇上，可来源是每天持续进文档的。
-- 换成显式开关：猜测没有了，冻结也没有了，"要不要替我做"由人声明。
ALTER TABLE knowledge_bases
    ADD COLUMN auto_extend_ontology BOOLEAN NOT NULL DEFAULT TRUE;

-- 记忆：dismiss 此前是 DELETE，下一次抽取遇到同一个词原样插回来——用户的
-- "不要"活不过一轮抽取。开关关着时这只是恼人；开着时它变成系统覆盖人的
-- 明确决定。标记而非删除，累加与自动采纳都绕开已标记的。
ALTER TABLE ontology_misses
    ADD COLUMN dismissed_at TIMESTAMPTZ;
