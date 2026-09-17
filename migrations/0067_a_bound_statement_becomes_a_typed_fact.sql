-- 绑上的签名下的开放陈述算成类型化事实（0044 决定 3 的第三片：类型化图谱是按签名绑定
-- 从开放图谱算出来的视图）。
--
-- 一条类型化行从哪条开放陈述算出来，记在 `from_statement_id` 上：谓词是绑定给的属性，
-- 主宾按绑定的方向，字面值、世界轴时间（0045 解算在开放陈述上的 valid_*）、来源时间、
-- 置信度、证据与限定都从那条陈述抄过来。绑定变了（重判、删了、方向反了）或陈述作废了，
-- 它名下的类型化行作废；绑定不变的行不动。带 mood 限定的陈述（要、预计、若）不算——
-- 文档没有断言它成立。
--
-- 老的类型化路径写的行（#736 之前）没有 from_statement_id，不受这一套管。

ALTER TABLE facts ADD COLUMN from_statement_id UUID REFERENCES facts(id) ON DELETE CASCADE;

-- 只有类型化行才从陈述算出来；开放陈述自己不指向别的陈述
ALTER TABLE facts ADD CONSTRAINT facts_from_statement_is_typed
    CHECK (from_statement_id IS NULL OR layer = 'typed');

CREATE INDEX facts_from_statement_idx ON facts (from_statement_id) WHERE from_statement_id IS NOT NULL;
