-- 判据动了，靠着它开着的结论随之收场（0062 / #564）。
--
-- `axiom_violations.status = 'open'` 的约定从这条起收紧成「已对账于**当前**
-- 本体」：`update_relation_type` 改掉公理或 domain / range 时，同一事务里
-- 重新检出；只在旧判据下还成立的 open 行不能删——删掉就把「这条违规是在
-- 哪条公理下结束」这件事抹了——也不能继续开着——开着就意味着当前本体还判它。
-- 给它们一个收场理由 `criterion_changed`：不是人裁的（decided_by 空），是判据
-- 换了。
--
-- 它在语义上是 `axiom_relaxed` 的邻铺：那条说的是「人承认这条公理写错了」，
-- 这条说的是「公理改了，这个发现不再成立」。两者都承诺「世界变了所以违规不该
-- 再算出来」——重开检查（`reasoning::run` 把 resolved 的破约行翻回 open）认它
-- 同样的话。

ALTER TABLE axiom_violations
    DROP CONSTRAINT axiom_violations_resolution_check,
    ADD CONSTRAINT axiom_violations_resolution_check CHECK (resolution IN (
        'fact_retracted', 'fact_closed', 'axiom_relaxed', 'accepted',
        'criterion_changed'
    ));

-- #861 起规则推出的关系边以临时 id 进前提链，`derived_contradiction` 的 `path`
-- 曾把不属于任何表的 id 落了库。落库的 path 是导出的证据链，成员必须回得到
-- 本库 `facts` 的行——把回不成的成员剔掉（顺序不动）
UPDATE axiom_violations v
   SET path = COALESCE((
       SELECT array_agg(m ORDER BY ord)
         FROM unnest(v.path) WITH ORDINALITY AS u(m, ord)
        WHERE EXISTS (SELECT 1 FROM facts f WHERE f.id = m AND f.kb_id = v.kb_id)
   ), '{}')
 WHERE EXISTS (
       SELECT 1 FROM unnest(v.path) AS m
        WHERE NOT EXISTS (SELECT 1 FROM facts f WHERE f.id = m AND f.kb_id = v.kb_id)
   );
