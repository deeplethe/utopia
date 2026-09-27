-- 人改过区间的行自己记着改的是哪一端（#970，#976 合并时的评审）。
--
-- #976 让事实行说出「这一段是人改过的」：顺着 supersedes 链找 `fact.time_corrected` 审计。
-- 那条审计是修正路由在改完之后尽力写的（写失败不回滚，拿不到快照就不写），它一丢，
-- 行上就不再说这一段是人改的，而模型会把人改的日期当成原句说的。人也可能只改了一端：
-- 另一端仍是原句说的，行上却只能说「改过」。
--
-- `corrected_ends` 与 `end_derived` 一样随行写：`correct_interval` 在修正的同一个事务里
-- 比较旧行与新区间，哪一端（值或精度）变了记哪一端，与旧行已有的取并集；关上、改终点、
-- 搬主宾、认领属性这些改写原样带过去。人交来的是整段区间，改完时间线不再重画它：原来是
-- 时间线推出来的终点，人没改值也是把它钉住了，算人的那一端。
--   start  人改过起点
--   end    人改过终点（或钉住了时间线推出来的终点）
--   both   两端都是人的
-- NULL 是没人改过。
--
-- 回填：这一列之前改过的行，从审计里算。审计记在被改的那一行（改之前的行）上，`detail`
-- 自 #311 起记着改前的区间（`from`）与改后的（`to`）；被改的那一行还在表里（作废，不删），
-- 它的终点是不是推出来的也看得到。从每一条审计的那一行顺着 supersedes 往下走，修正行和
-- 它之后改写出来的每一行都算上这一次改了哪端，几次取并集。只填还空着的行，再跑一遍什么
-- 都不改。
ALTER TABLE facts ADD COLUMN IF NOT EXISTS corrected_ends TEXT
    CHECK (corrected_ends IN ('start', 'end', 'both'));

WITH RECURSIVE down (fact_id, audit_id) AS (
    SELECT f.id, a.id
      FROM audit_events a
      JOIN facts f ON f.supersedes = a.target_id
     WHERE a.action = 'fact.time_corrected' AND a.target_id IS NOT NULL
    UNION
    SELECT f.id, down.audit_id
      FROM facts f
      JOIN down ON f.supersedes = down.fact_id
),
changed AS (
    SELECT down.fact_id,
           bool_or(a.detail->'from' IS NULL OR a.detail->'to' IS NULL
                   OR a.detail->'from'->'valid_from' IS DISTINCT FROM a.detail->'to'->'valid_from'
                   OR a.detail->'from'->'valid_from_precision'
                      IS DISTINCT FROM a.detail->'to'->'valid_from_precision') AS start_changed,
           bool_or(a.detail->'from' IS NULL OR a.detail->'to' IS NULL
                   OR a.detail->'from'->'valid_to' IS DISTINCT FROM a.detail->'to'->'valid_to'
                   OR a.detail->'from'->'valid_to_precision'
                      IS DISTINCT FROM a.detail->'to'->'valid_to_precision'
                   OR COALESCE(t.end_derived, false)) AS end_changed
      FROM down
      JOIN audit_events a ON a.id = down.audit_id
      LEFT JOIN facts t ON t.id = a.target_id
     GROUP BY down.fact_id
)
UPDATE facts f
   SET corrected_ends = CASE WHEN c.start_changed AND c.end_changed THEN 'both'
                             WHEN c.start_changed THEN 'start'
                             ELSE 'end' END
  FROM changed c
 WHERE f.id = c.fact_id
   AND f.corrected_ends IS NULL
   AND (c.start_changed OR c.end_changed);
