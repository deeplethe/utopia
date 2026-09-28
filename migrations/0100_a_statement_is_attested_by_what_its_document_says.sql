-- 0064 · cut 3：陈述的见证来自文档自己说的日期，没有就是没有。
--
-- 从前开放陈述入库时 `attested_from` 填 `COALESCE(文档日期, now())`，而抽取跑在定日期之前，
-- 文档日期那时总是空的——于是每条陈述的见证都是处理文档的那一刻。读的人拿没起点的事实的
-- 下界时用的是 `COALESCE(valid_from, attested_from)`：一份 2020 年的报告今天传上来，它的
-- 事实读作「今天才有」，时间轴拉回去全不见了（#987；#714 说上传时刻不许进世界时间）。
--
-- 现在：见证是陈述所在那一节里文本说话的那一刻（0064 决定 3，抽取时各块报上来的日期里管着
-- 它的那个 `now`），连同那条日期的名字；文档没说就留空，留空的行在任何时点都成立（决定 5）。
-- 人此刻写下的事实照旧由此刻作证：这一列的缺省值不动，只是允许为空。
ALTER TABLE facts ALTER COLUMN attested_from DROP NOT NULL;
-- 见证从哪条日期来，照文档的字：「提报日期 2026年9月4日」
ALTER TABLE facts ADD COLUMN attested_by TEXT;

-- 已有的开放陈述：锚点挪到最早的带日期证据上（日期来自正文或来源系统才算，证据所在
-- 那一版的日期优先，与 `temporal::DATED_AT` 同一口径）；一份带日期的证据都没有的留空。
-- 它们原来的值是处理文档的时刻，不是任何文档说的日期
UPDATE facts f
   SET attested_from = (
        SELECT min(COALESCE(v.doc_time, d.doc_time))
          FROM fact_evidence fe
          JOIN documents d ON d.id = fe.document_id
     LEFT JOIN document_versions v ON v.document_id = fe.document_id
                                  AND v.version = fe.doc_version
         WHERE fe.fact_id = f.id AND d.deleted_at IS NULL
           AND d.doc_time_source IN ('content', 'source')
           AND COALESCE(v.doc_time, d.doc_time) IS NOT NULL)
 WHERE f.layer = 'open';

-- 类型化的行自己没有出处，跟着物化它的那些陈述走：来源里最早的那个见证，都没有就留空
UPDATE facts t
   SET attested_from = src.at
  FROM (SELECT ts.fact_id, min(s.attested_from) AS at
          FROM typed_fact_sources ts
          JOIN facts s ON s.id = ts.statement_id
         GROUP BY ts.fact_id) src
 WHERE t.id = src.fact_id AND t.layer = 'typed'
   AND t.attested_from IS DISTINCT FROM src.at;
