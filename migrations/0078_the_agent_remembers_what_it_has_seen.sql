-- 0061 · cut 1.1：代理记得自己看过什么。
--
-- 第一次真跑（bench4，100 篇）：送去 120 条形状，35 条进了提案，其余 85 条没有任何记录，
-- 下一轮按陈述数排序又是它们排在最前——每轮七成 token 花在模型已经看过、已经决定不提的
-- 形状上。同一轮里模型答"本体里已经有"的（陈述最多的那些：died in → placeOfDeath、
-- wrote → author）也被丢掉了，而那正是审核队列要的判断。
--
-- 每条送去的形状记一行：提了、已有、没提。提了的不再送（采纳后由对齐重判，拒绝了人已说过）；
-- 已有与没提的，等词表变了再送——basis 是看它时词表（类键 + 属性键）的指纹。
CREATE TABLE ontology_agent_reviews (
    kb_id       UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- phrase | kind_word
    kind        TEXT NOT NULL CHECK (kind IN ('phrase', 'kind_word')),
    -- phrase: {"phrase","subject","object","value"}；kind_word: {"kind_word"}
    shape       JSONB NOT NULL,
    -- proposed | existing | declined
    outcome     TEXT NOT NULL CHECK (outcome IN ('proposed', 'existing', 'declined')),
    -- proposed: 提案的 section:key；existing: 已有元素的键
    target      TEXT,
    -- 看它时词表的指纹
    basis       TEXT NOT NULL,
    reviewed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kb_id, kind, shape)
);
