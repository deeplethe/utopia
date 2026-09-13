-- 环按整条路径定键（#641）。
--
-- `axiom_violations` 从 0012 起按 `(kb_id, kind, left_fact, right_fact)` 唯一。环的
-- left 是最小的那条事实、right 是转到它起头之后的最后一条（0052）——两个环只要共用
-- 这两条、中间走的不同，就落在同一个键上，后算出来的覆盖先算出来的 `path`。量过：
-- 400 个单位的组织重组库上形状环 572 个，落库 165 行。被吞掉的环在 Review 里看不到，
-- 留下的是哪一条取决于遍历顺序，人裁的那一个可能在下一轮换成了另一个。
--
-- 一个环就是它的事实集合。节点不重复的回路里，集合定下来顺序就定了，转到最小事实
-- 起头之后 `path` 本身就是规范形状，所以环按 `(kb_id, path)` 唯一。别的种类不变：
-- 互斥组的首尾本来就是组的函数（0053），派生矛盾有意把同一处撞法的不同前提算作一处
-- （#619）。

-- 很早记下的环行可能没有 path（path 列是后加的）。首尾两条在 0052 之后是唯一的，
-- 补成它们，免得几行空 path 在新索引上撞在一起
UPDATE axiom_violations
   SET path = ARRAY[left_fact, right_fact]
 WHERE kind = 'cycle' AND cardinality(path) = 0;

ALTER TABLE axiom_violations
    DROP CONSTRAINT axiom_violations_kb_id_kind_left_fact_right_fact_key;

CREATE UNIQUE INDEX axiom_violations_pair_key
    ON axiom_violations (kb_id, kind, left_fact, right_fact)
    WHERE kind <> 'cycle';

CREATE UNIQUE INDEX axiom_violations_cycle_key
    ON axiom_violations (kb_id, path)
    WHERE kind = 'cycle';
