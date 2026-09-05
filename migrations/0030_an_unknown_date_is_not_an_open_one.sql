-- 0022：未知的日期不是开放的日期。
--
-- 写入侧分得清「仍在持续」（valid_to NULL，精度 NULL）与「结束了，不知哪天」
-- （valid_to NULL，精度 'unknown'），也从不替原文编一个起点。读出侧把两者都读回
-- 「随时成立」。修法不改这张表上任何一列的含义，而是补一列说清自己是什么：
--
-- attested_at：这一行的各次观察里，**最早那份文档的日期**。没有起点的事实从它起
-- 成立；结束了不知哪天的事实到它为止。它是世界轴上的时刻（文档的日期），不是
-- recorded_at（我们何时写下——另一根轴，0019）：回填的语料一个晚上灌进 2023–2025
-- 年的文档，按 recorded_at 锚，每条没起点的事实都会出现在 2026 年、而在任何一个
-- 历史时刻都不出现。
--
-- 不把文档日期填进 valid_from / valid_to（0003 的拒绝仍然成立）：那两列写的是原文
-- 说了什么，读它们的人不该先查精度才敢信。
ALTER TABLE facts ADD COLUMN attested_at TIMESTAMPTZ;

-- 回填：证据里最早的文档日期；没有证据的行（人手录的）按记下的时刻。
UPDATE facts f
   SET attested_at = COALESCE(
       (SELECT min(COALESCE(d.doc_time, d.created_at))
          FROM fact_evidence fe JOIN documents d ON d.id = fe.document_id
         WHERE fe.fact_id = f.id),
       f.recorded_at);

-- 默认 now()：人此刻写下的事实，人就是证据。写入路径都显式给值（抽取给文档日期，
-- 修正行从被替代的行继承），默认值兜住的是测试里直接 INSERT 的行。
ALTER TABLE facts
    ALTER COLUMN attested_at SET NOT NULL,
    ALTER COLUMN attested_at SET DEFAULT now();
