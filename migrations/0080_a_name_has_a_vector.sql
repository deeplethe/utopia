-- 一个名字有一条向量（0041 决定 3 的第二条召回通道，第 2 刀）。
--
-- 召回今天只认字面：mention 的名字（及其泛用后缀变体）与名字事实精确相等才成候选。
-- 于是 海探1 在一篇从没写过全名的文档里谁也碰不上，一个名字写成两种文字也永远
-- 是两个实体（#709）。名字事实本身是对的（0041 决定 1），缺的是「相近的名字也来
-- 报个到」这一条路——名字字符串的向量，同类里取最近的几条。
--
-- 存法：一张从表，一条名字事实一行。不放进 `facts` 加列——那张表 `SELECT *` 进
-- `Fact` 的地方太多，为万分之一的行加一列全表都要跟着动；也不放进 `entities`——
-- 一个实体有几个名字就该有几条向量，简称和全名各算各的。`embedding` 不定维，随
-- 所选嵌入模型（与 `chunks.embedding` 同一条规矩）；HNSW 由 `vector_index` 按第一次
-- 写下的维度排任务去建，查询照 0035 的两条规矩写。
--
-- 同库不变量（0070 §1b）：行自己带 kb_id，复合外键把「事实存在」和「事实同库」
-- 合成一条约束；kb_id 落定后不改（0070 的通用触发器）。名字事实作废（invalidated_at）
-- 时向量留着，查询那头按事实是否现行过滤——作废是可撤的，向量不必重算。
CREATE TABLE name_vectors (
    fact_id    UUID PRIMARY KEY,
    kb_id      UUID NOT NULL,
    entity_id  UUID NOT NULL,
    embedding  vector NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT name_vectors_fact_same_kb
        FOREIGN KEY (kb_id, fact_id) REFERENCES facts (kb_id, id) ON DELETE CASCADE,
    CONSTRAINT name_vectors_entity_same_kb
        FOREIGN KEY (kb_id, entity_id) REFERENCES entities (kb_id, id) ON DELETE CASCADE
);

-- 合并搬名字事实时（0041：名字随合并走、撤回搬回），向量跟着事实走，entity_id 得
-- 能改；只有库不能改
CREATE TRIGGER name_vectors_keep_their_kb
    BEFORE UPDATE OF kb_id ON name_vectors
    FOR EACH ROW EXECUTE FUNCTION kb_ownership_is_not_reassigned();

-- 按实体找它的名字向量（合并搬动、面板展示）
CREATE INDEX name_vectors_entity_idx ON name_vectors (kb_id, entity_id);
