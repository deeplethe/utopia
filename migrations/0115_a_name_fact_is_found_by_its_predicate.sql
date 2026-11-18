-- 按谓词找现行的名字事实（#1296）。
--
-- 补向量每篇文档都跑一次 `name_vectors::pending`：现行的、`known_as` 上的、还没有向量的名字
-- 事实，按 id 取一批。从前没有带谓词的索引，它只能沿 `facts_kb_id_key` 把整个库的事实按 id
-- 读一遍再筛谓词——20 万条事实的库里每篇文档读 20 万行，为的是剩下的三五个名字。
--
-- `id` 列让索引按谓词定位之后仍按 id 排好，`ORDER BY f.id LIMIT` 顺着走、不用排序，只读这个库
-- 的名字事实。谓词在查询里是一个子查询的值，见 `pending` 上的说明。
CREATE INDEX IF NOT EXISTS facts_live_predicate_idx ON facts (kb_id, predicate_id, id)
  WHERE invalidated_at IS NULL;
