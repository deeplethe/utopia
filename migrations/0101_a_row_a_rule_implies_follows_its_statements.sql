-- 0064 决定 5 的补遗：规则算出来的行也跟着它读的陈述作证。
--
-- 0100 让没说时间的事实在任何时点都成立，物化出来的类型化行跟着来源陈述走。规则算的
-- 隐含行（0073）没跟上：入库时陈述没有见证，写入口就填处理那一刻（那是给人写的事实留的
-- 缺省）；类别词规则从实体算的行本来就没有见证，也成了处理那一刻；陈述后来由时间解析
-- 作了证，它们也不跟。于是同一条没说时间的陈述，绑定物化的那行任何时点都成立，规则算的
-- 那行只从处理那一刻起成立。
--
-- 现在物化与时间解析同步见证时，规则算的行也在内（`materialize::sync_typed_attestation`）。
-- 这里把已有的行按同一条规矩补一次：来源陈述（物化进来的、规则读的）里最早的那个见证，
-- 连同那条日期的名字，一条都没有就留空；人写的行只被规则的结论并进来的不动。与 0100 一样
-- 不按库筛，作废的行也在内
UPDATE facts t
   SET attested_from = src.at, attested_by = src.by
  FROM (SELECT u.fact_id,
               min(s.attested_from) AS at,
               (array_agg(s.attested_by ORDER BY s.attested_from NULLS LAST, s.id))[1] AS by
          FROM (SELECT fact_id, statement_id FROM typed_fact_sources
                UNION ALL
                SELECT fact_id, statement_id FROM implied_fact_sources) u
     LEFT JOIN facts s ON s.id = u.statement_id
         GROUP BY u.fact_id) src
 WHERE t.id = src.fact_id AND t.layer = 'typed'
   AND (t.implied OR EXISTS (SELECT 1 FROM typed_fact_sources ts WHERE ts.fact_id = t.id))
   AND (t.attested_from IS DISTINCT FROM src.at OR t.attested_by IS DISTINCT FROM src.by);
