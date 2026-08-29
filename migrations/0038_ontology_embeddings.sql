-- 本体自己的向量。类、关系、属性各一条，用 label + description 嵌出来。
--
-- 为什么要它：今天凡是要问"这个说法对应本体里的哪一个"的地方，都是把整个本体
-- 内联进提示词——build_proposals 一次塞 1949 个 key（还只有 key 没有描述，
-- 模型据此根本判断不了"这是不是已有类型的同义说法"）。有了向量就能只取候选。
--
-- 维度不定：跟 chunks.embedding 一样随工作区所选模型走，所以不建 HNSW，顺扫。
-- 本体行数以千计，顺扫完全够。
ALTER TABLE entity_types   ADD COLUMN embedding vector;
ALTER TABLE relation_types ADD COLUMN embedding vector;

-- **存"当时嵌的是什么"而不是一个时间戳。**
-- 时间戳只能回答"嵌过没有"，回答不了"嵌的还是不是现在这段文字"——描述改了、
-- 嵌入模型换了，向量就是陈的，而时间戳看不出来。存原文与模型名，填充任务
-- 一比对就知道该重嵌谁，也就不必去每一个改描述的写入点挂钩子（漏一个就悄悄烂掉）。
-- 本体只有几千行，存原文的代价可以忽略，而排查检索质量时它值钱。
ALTER TABLE entity_types   ADD COLUMN embedded_text text, ADD COLUMN embedded_model text;
ALTER TABLE relation_types ADD COLUMN embedded_text text, ADD COLUMN embedded_model text;
