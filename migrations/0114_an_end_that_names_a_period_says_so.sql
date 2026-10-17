-- 终点说的是它之前那个时段时，行上记一笔（#994，0053 的修订）。
--
-- 「works for Meridian from 2024-03-01」再加「left in 2024」，#975 之后存成 2024-03-01 →
-- 2025-01-01（年精度）：世界轴上这是对的，2024 年全年在职，2025 年不在。可「until 2025」
-- 关上同一行，存的也是 2025-01-01（年精度）。只看值分不出原文说的是 2024 还是 2025，读者
-- 只能按存的时刻印，印出来是下一个时段。
--
-- `end_after_period`：终点是原文所说时段之后的那一刻。存的时刻不变，世界轴照旧读它，读者
-- 据此印原文说的时段。写入路径在 #975 挪终点的两处设它（收尾、精化）；一条开着的行被
-- 「until 2025」与「left in 2024」两句关上，哪一句先到都设；改写、搬主宾、勘误改出来的行
-- 带着它；时间线画的终点、不知哪天的终点从不带。约束要求有起点：终点只在起点落在那个时段
-- 里时才被挪到时段的尽头，没有起点的行第一刀不标。第二刀让时间解析自己写这种终点时，按
-- 名字换掉这条约束，去掉起点那一条。
--
-- 回填：只标活着的状态行，它的起点落在所说的时段里，而且有一条来源陈述在同一精度上早
-- 一个单位说出了终点——没有起点的「至」，或绑定标为 end 的单个日期（与物化的 MATCH
-- 同一个匹配）。标为 start 的单个日期不算：「joined in 2024」加「until 2025」说的是到
-- 2025 年。绑定在 start 与 end 之间翻转时已经算出的行留着（0053），回填读的却是绑定现在
-- 的值，所以单个日期同时给了这一行的起点时，要另有一条来源陈述给出同一个起点才算：不然
-- 「joined」翻成 end 之后，上面那行会被标上；#994 自己的同日例子（「from 2023-06-01」加
-- 「left on 2023-06-01」）照样标。另一条同样标为 end 的单个日期不算数：同一句话可能存成
-- 两条陈述（短语的大小写、空白不同，或合并实体之后主宾相同），翻转之后两条都标 end。
-- 人改过终点的、时间线画的、没有陈述来源的（点头写的、人写的）、被后来的起点精化过的
-- 蕴含行（它的来源留在旧行上）、已作废的都不标，照旧印下一个时段，人在对话框里改一次即可。
--
-- 约束先删再加，整份文件可以重放；约束在回填之前，回填写下的每一行都受它检查。
-- `NOT f.end_after_period` 让再跑一遍什么都不改。时段的开头用 date_trunc(.., 'UTC') 算，
-- 与会话的时区无关。
ALTER TABLE facts ADD COLUMN IF NOT EXISTS end_after_period BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE facts DROP CONSTRAINT IF EXISTS facts_end_after_period_needs_a_stated_end;
ALTER TABLE facts ADD CONSTRAINT facts_end_after_period_needs_a_stated_end
  CHECK (NOT end_after_period OR (valid_from IS NOT NULL AND valid_to IS NOT NULL AND NOT end_derived));

UPDATE facts f SET end_after_period = TRUE
  FROM relation_types r
 WHERE r.id = f.predicate_id AND r.temporal = 'state'
   AND f.layer = 'typed' AND f.invalidated_at IS NULL
   AND NOT f.end_after_period AND NOT f.end_derived
   AND (f.corrected_ends IS NULL OR f.corrected_ends = 'start')
   AND f.valid_from IS NOT NULL AND f.valid_to IS NOT NULL
   AND f.valid_from < f.valid_to
   AND date_trunc(f.valid_to_precision, f.valid_to - interval '1 microsecond', 'UTC') <= f.valid_from
   AND EXISTS (
     SELECT 1
       FROM (SELECT t.statement_id FROM typed_fact_sources t WHERE t.fact_id = f.id
             UNION ALL
             SELECT i.statement_id FROM implied_fact_sources i
              WHERE i.fact_id = f.id AND i.statement_id IS NOT NULL) src
       JOIN facts s ON s.id = src.statement_id
      WHERE s.valid_to = date_trunc(f.valid_to_precision, f.valid_to - interval '1 microsecond', 'UTC')
        AND s.valid_to_precision = f.valid_to_precision
        AND (s.valid_from IS NULL                       -- 至：只有终点
             OR (s.valid_from = s.valid_to              -- 单个日期，绑定标为 end
                 AND EXISTS (SELECT 1 FROM phrase_bindings b
                               JOIN entities se ON se.id = s.subject_id
                          LEFT JOIN entities oe ON oe.id = s.object_id
                              WHERE b.kb_id = s.kb_id
                                AND b.phrase = lower(btrim(regexp_replace(s.phrase, '\s+', ' ', 'g')))
                                AND b.subject_type_id IS NOT DISTINCT FROM se.type_id
                                AND b.object_is_value = (s.object_id IS NULL)
                                AND (s.object_id IS NULL OR b.object_type_id IS NOT DISTINCT FROM oe.type_id)
                                AND b.relation_type_id = f.predicate_id
                                AND b.status = 'bound' AND b.marks = 'end')
                 -- 这个单个日期若也给了这一行的起点，只有另一条来源陈述给出同一个起点时才算；
                 -- 另一条同样标为 end 的单个日期不算（见文件头）
                 AND NOT (s.valid_from = f.valid_from
                          AND s.valid_from_precision IS NOT DISTINCT FROM f.valid_from_precision
                          AND NOT EXISTS (SELECT 1 FROM typed_fact_sources t2
                                            JOIN facts o ON o.id = t2.statement_id
                                           WHERE t2.fact_id = f.id AND o.id <> s.id
                                             AND o.valid_from = f.valid_from
                                             AND o.valid_from_precision IS NOT DISTINCT FROM f.valid_from_precision
                                             AND NOT (o.valid_to IS NOT DISTINCT FROM o.valid_from AND EXISTS (
                                               SELECT 1 FROM phrase_bindings b2
                                                 JOIN entities se2 ON se2.id = o.subject_id
                                            LEFT JOIN entities oe2 ON oe2.id = o.object_id
                                                WHERE b2.kb_id = o.kb_id
                                                  AND b2.phrase = lower(btrim(regexp_replace(o.phrase, '\s+', ' ', 'g')))
                                                  AND b2.subject_type_id IS NOT DISTINCT FROM se2.type_id
                                                  AND b2.object_is_value = (o.object_id IS NULL)
                                                  AND (o.object_id IS NULL OR b2.object_type_id IS NOT DISTINCT FROM oe2.type_id)
                                                  AND b2.relation_type_id = f.predicate_id
                                                  AND b2.status = 'bound' AND b2.marks = 'end')))))));
