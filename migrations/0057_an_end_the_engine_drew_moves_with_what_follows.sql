-- 一行的终点是谁定的：时态引擎按时间线推出来的，还是原文、人写下的（#679 评审）。
--
-- 引擎把一段开放的值关在下一段开始时。那个终点没有哪份文件说过，它只是「后面那一段
-- 从这天起」的影子：后面那一段挪了（晚到的证据把它的日期往前挪、补上了起点）、走了
-- （撤回合并、删了文档），这个终点就该跟着变。原文写明的终点、人改过的区间一律不重算——
-- 两份原文说法相左时，该由人来挑。
--
-- 从前靠「终点正好等于另一行的起点」认出引擎画的界，后面那一段一挪就认不出来，界留在
-- 原地，两段叠在一起。现在由写这一行的人记下：引擎写的为真，其余为假。
ALTER TABLE facts ADD COLUMN end_derived BOOLEAN NOT NULL DEFAULT FALSE;

-- 回填：迁移之前引擎关上的行。不回填的话它们一律算写明的，永远不重算——已有的库再来两份
-- 晚到的补充协议，照样叠在一起。
--
-- 只认得出来的那种，宁可漏：一条现存的闭合行，
--   · 谓词是声明了唯一性的状态关系；
--   · 它改写的前身还开着，起点、值都与它相同——引擎关一行只换终点；
--   · 它的终点正好是同一条时间线上另一个值的起点——引擎只关在那里；
--   · 它的证据没有前身之外的——原文说出终点的那次观察会带来新证据；
--   · 前身上没有人的决定：没被人裁决关上（fact_conflicts 的 closed），没被人手动关上或改过
--     时间（审计里的 fact.close / fact.time_corrected）。
UPDATE facts c
   SET end_derived = TRUE
  FROM facts p, relation_types r
 WHERE p.id = c.supersedes
   AND r.id = c.predicate_id
   AND r.temporal = 'state' AND (r.functional OR r.inverse_functional)
   AND c.invalidated_at IS NULL
   AND c.valid_to IS NOT NULL
   AND p.valid_to IS NULL AND p.valid_to_precision IS NULL
   AND p.valid_from IS NOT DISTINCT FROM c.valid_from
   AND p.subject_id = c.subject_id
   AND p.predicate_id = c.predicate_id
   AND p.object_id IS NOT DISTINCT FROM c.object_id
   AND p.object_value IS NOT DISTINCT FROM c.object_value
   AND EXISTS (
         SELECT 1 FROM facts o
          WHERE o.kb_id = c.kb_id AND o.predicate_id = c.predicate_id
            AND o.valid_from = c.valid_to
            AND ((r.functional AND o.subject_id = c.subject_id
                  AND (o.object_id IS DISTINCT FROM c.object_id
                       OR o.object_value IS DISTINCT FROM c.object_value))
              OR (r.inverse_functional AND c.object_id IS NOT NULL
                  AND o.object_id = c.object_id AND o.subject_id <> c.subject_id)))
   AND NOT EXISTS (
         SELECT 1 FROM fact_evidence ce
          WHERE ce.fact_id = c.id
            AND NOT EXISTS (SELECT 1 FROM fact_evidence pe
                             WHERE pe.fact_id = p.id AND pe.chunk_id = ce.chunk_id))
   AND NOT EXISTS (
         SELECT 1 FROM fact_conflicts x
          WHERE x.old_fact_id = p.id AND x.resolution = 'closed')
   AND NOT EXISTS (
         SELECT 1 FROM audit_events a
          WHERE a.target_id = p.id AND a.action IN ('fact.close', 'fact.time_corrected'));
