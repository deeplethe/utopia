-- 同一个三元组由几条陈述算出来时，类型化图谱里只有一行（0044 决定 3 的第四片）。
--
-- 0067 让每条绑上的陈述各成一行类型化事实，于是两份文档都说「Harbor Bakery 在 Port Ellen」
-- 时间线上就有两条一模一样的边。现在物化走类型化图谱本来的门（`insert_fact` /
-- `insert_value_fact`）：同断言同起点的复用那一行、只是又被提到一次；没时间的裸行被带
-- 时间的观察取代并链上（supersedes）；「结束了」关上开着的行。一行于是可能来自几条陈述，
-- 来源记在 `typed_fact_sources` 里，一条陈述一行；`facts.from_statement_id` 留作第一条
-- 来源。作废的规则跟着变：一行来源全都不成立了才作废；某条来源的绑定变了，只删它那条
-- 来源记录。

CREATE TABLE typed_fact_sources (
    fact_id       UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    statement_id  UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    PRIMARY KEY (fact_id, statement_id)
);
CREATE INDEX typed_fact_sources_statement_idx ON typed_fact_sources (statement_id);

-- 0067 已经算出来的行：它的那一条陈述就是它的来源
INSERT INTO typed_fact_sources (fact_id, statement_id)
SELECT id, from_statement_id FROM facts
 WHERE from_statement_id IS NOT NULL
ON CONFLICT DO NOTHING;
