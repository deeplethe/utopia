-- 改类要能说出是谁改的。
--
-- `entity_retypes` 记了改动本身（何时、从哪个类到哪个类、撤没撤），唯独没记人。
-- 而这张表马上要出现在实体历史里（0001 P3a：「该补的是让改类在实体历史里显形」），
-- 一条写着「Vehicle ← 未分类」却不说谁干的记录，比不显示更容易被误读成引擎所为。
--
-- 可空 = 引擎自动裁决，跟 `entity_merges.merged_by` 已有的约定一字不差
-- （那边的注释写着「NULL = 自动合并（LLM 裁决高置信）」）。不给默认值：
-- 已有的行确实不知道是谁，编一个出来就是伪造归因。
ALTER TABLE entity_retypes ADD COLUMN actor_id UUID REFERENCES users(id);
