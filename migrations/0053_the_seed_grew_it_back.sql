-- 0052 删掉了 `related_to`，代码七分钟后把它种了回来。
--
-- 迁移改的是**数据**，而种子关系是**代码**：`graph.rs` 的 `DEFAULT_RELATION_TYPES`
-- 里那一行还在，`ensure_default_ontology` 在建库、看本体页、**每次抽取**时都会跑，
-- 于是 `ON CONFLICT (kb_id, key)` 找不到行就重新插一条。实测账本：
--
--     0052 应用于 08-30 17:54   →  全库 related_to 归零
--     bench demo-autoextend     →  08-30 18:01 又长出一条 builtin=true
--
-- 比"没删干净"更糟的是第二层：0052 同时删掉了抽取提示词里那条排除过滤
--（当时的理由是"行都没有了，不需要记得别列它"）。行既然还在，结果就是
-- `related_to` **第一次被列进提示词给模型看**。0001 量过这件事的代价：
-- 359 次使用里 321 次是模型从清单上挑的，只有约 38 次是代码降级——
-- 逃生舱一旦摆上台面，模型就不再去说原文究竟说了什么。
--
-- 代码那半边这次一起改了（种子表与中文表里的 related_to 都已删除）。
-- 这条迁移只负责把已经长回来的行清掉。
--
-- **只删 builtin 的。** 谁想自己建一个叫 related_to 的关系是他的自由——
-- 那是一个人做的决定，与这里要消灭的"程序状态伪装成词汇"是两回事。

UPDATE facts SET predicate_id = NULL
 WHERE predicate_id IN (SELECT id FROM relation_types WHERE key = 'related_to' AND builtin);

DELETE FROM relation_types WHERE key = 'related_to' AND builtin;
