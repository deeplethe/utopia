-- 派生撞上断言时，让路这件事从静默变成可见（docs/decisions/0017）。
--
-- 0002 定了 asserted > derived：推出来的事实撞上账本里的断言就不落地。此前那一步
-- 什么都不留——`ceo_of ⊑ works_at` 推出的 works_at 没了，人不知道有过这回事，也就
-- 不知道该去看看是抽取错了、旧断言该闭合、还是两个「Mira」其实是一个人。
--
-- 一致性检查多一种 `derived_contradiction`：left 是被撞的断言，right 是派生的最后一条
-- 前提，path 是全部前提；推出来的三元组本身没有落库、没有 id 可指，放进 `detail`。
-- 出路多一条 `fact_closed`——最常见的修法是给旧断言一个结束日期。
--
-- 派生之间互撞（两条规则加在一起产出互斥的结论）按规则对聚合进 `ontology_defects`，
-- 一种 `rules_disagree`，`detail` 记规则对与几个例子。逐对进 Review 只会淹掉队列。

ALTER TABLE axiom_violations
    DROP CONSTRAINT axiom_violations_kind_check,
    ADD CONSTRAINT axiom_violations_kind_check CHECK (kind IN (
        'self_loop', 'asymmetry', 'cycle', 'functional', 'signature',
        'derived_contradiction'
    )),
    DROP CONSTRAINT axiom_violations_resolution_check,
    ADD CONSTRAINT axiom_violations_resolution_check CHECK (resolution IN (
        'fact_retracted', 'fact_closed', 'axiom_relaxed', 'accepted'
    )),
    ADD COLUMN detail JSONB NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE ontology_defects
    DROP CONSTRAINT ontology_defects_kind_check,
    ADD CONSTRAINT ontology_defects_kind_check CHECK (kind IN (
        'symmetric_and_asymmetric', 'transitive_and_functional', 'subclass_cycle',
        'disjoint_with_ancestor', 'inherits_disjoint',
        'inverse_of_itself', 'inverse_not_mutual', 'sub_property_cycle',
        'rules_disagree'
    )),
    ADD COLUMN detail JSONB NOT NULL DEFAULT '{}'::jsonb;
