-- 一条短语签名的文本只嵌一次（#1097）。
--
-- 短语对齐给候选多的签名开短名单，要把签名的文本（短语 · 例句 · 引文）嵌入，再与属性的向量
-- 比近。这些文本在两轮之间几乎不变，从前每轮全部重嵌：一个一万七千条候选多的签名的库，
-- 每轮一万七千次嵌入，要判的可能只是新来的几十条，嵌入成了一轮的固定开销，随库线性长。
--
-- 存法：按 (库, 嵌入模型, 文本的 SHA-256) 存向量，一轮只嵌没见过的文本，并删掉这一轮不再
-- 出现的哈希、以及别的模型嵌的行。模型在键里，换了模型旧行自然用不上。`embedding` 不定维，
-- 随所选嵌入模型（与 `chunks.embedding` 同一条规矩）；只按键读、不做近邻查询，所以不建
-- HNSW、不登记 `vector_index`。这是缓存不是账本——删了下一轮重嵌，结果一样——所以不进
-- 0070 的同库责任面，也不随库导出。
CREATE TABLE signature_vectors (
    kb_id     UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    model     TEXT NOT NULL,
    text_hash BYTEA NOT NULL,
    embedding vector NOT NULL,
    PRIMARY KEY (kb_id, model, text_hash)
);
