-- 签名违规成为一致性检查的一种（见 #190 / #196，docs/decisions/0012 待做第一条）。
--
-- #138 在抽取写入时按关系的 domain 掰正方向：主语不合、宾语合就对调，都不合就
-- 留空谓词。但写谓词的路不止抽取一条：**采纳**把谓词挂回旧事实（#190，实测把
-- 违反率从 0 抬到 12.3%），**合并**换掉主语的类型（#196）。守卫只在一条路上，
-- 另外两条各自绕过去了。
--
-- 修法两层：写入时的判断抽成一处（`ontology::judge_direction`），抽取与采纳共用；
-- 账本层再加一道兜底——一致性检查（0002 R0）多查一种 `signature`：活事实的主语
-- 不在谓词声明的 domain 里、或宾语不在 range 里。合并之后对搬动过的事实立刻查一遍，
-- 手动跑检查时全量查。**任何一条路写反了，人都能在 Review 里看见**，出路与其它
-- 违规一样：撤事实、放宽公理（去掉那条 domain 声明）、或认可并存。
--
-- 只动 CHECK 约束：表的形状够用——签名违规只涉及一条事实，left 与 right 同一条，
-- 与自反那类同款。
ALTER TABLE axiom_violations DROP CONSTRAINT axiom_violations_kind_check;
ALTER TABLE axiom_violations ADD CONSTRAINT axiom_violations_kind_check
    CHECK (kind IN ('self_loop', 'asymmetry', 'cycle', 'functional', 'signature'));
