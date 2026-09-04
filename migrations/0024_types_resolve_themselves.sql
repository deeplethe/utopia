-- 类型消解排队自动跑（0016 的 C2，0001 P3a 的收尾）。
--
-- 此前类型消解只在有人点的时候跑：新文档抽完，实体带着粗类型躺在图里，直到谁想起来
-- 去 Ontology 页点一下。现在抽取结束自动排一个 resolve_types 任务，只自动落地最安全的
-- 一档（在原类的子树里精化）；跨轴的改判仍留给人。
--
-- 引擎看过的实体打上 type_resolved_at：自动跑只挑没看过的，否则候选查询按事实数排序，
-- 每一轮都是同一批六十个，后面的永远轮不到。人点的那条路不看这个标记——人要的是重新
-- 审一遍。
ALTER TABLE entities ADD COLUMN type_resolved_at TIMESTAMPTZ;
-- 缺省开：在 ai-timeline × schema.org 上量过，自动落地那一档对 Wikidata 答案卷的命中
-- 39/41（两个"错"里一个是答案卷写窄了）；它只往子树里走一格，每一批都可撤
ALTER TABLE knowledge_bases ADD COLUMN auto_type_resolution BOOLEAN NOT NULL DEFAULT TRUE;
