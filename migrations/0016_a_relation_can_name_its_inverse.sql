-- 关系的逆与父属性（R1 的后两种规则源，见 docs/decisions/0002）。
--
-- ADR 0002 给 R1 定的规则来源是四种：`TransitiveProperty` / `SymmetricProperty` /
-- `inverseOf` / `subPropertyOf`。前两种一直在跑，后两种**连列都没有**——
-- 0013 的 `rules.kind` 注释把这件事记下来了：
--
-- > `inverseOf` 与 `subPropertyOf` 投影侧还没落库，所以也就编不出来
--
-- 这条迁移补上投影侧。缺的后果不是「少推几条」，是**答案不对称**：本体声明了
-- `works_at⁻¹ = employs`，而问「谁在 Acme 工作」和「Acme 雇了谁」会得到不同答案，
-- 除非两个方向都被断言过——那正是 R1 该消灭的重复劳动。
ALTER TABLE relation_types
    -- `p⁻¹ = q`。**单向存，双向用。**
    --
    -- 不加触发器去自动回填 `q.inverse_of = p`：那会把一条语义规则藏进数据库，
    -- 而绕过它的路不止一条（RDF 导入、直接 SQL）。改在读取公理时归一化——
    -- 载入那一处认这件事，绕不过去，也测得动（`reasoning::axioms_of`）。
    ADD COLUMN inverse_of UUID REFERENCES relation_types(id) ON DELETE SET NULL,
    -- `p ⊑ q`：断言了具体的，通用的也成立（`ceo_of ⊑ works_at`）。
    -- 链要防成环，R0 那边加检查——与 `entity_types` 的父类环是同一类问题
    ADD COLUMN sub_property_of UUID REFERENCES relation_types(id) ON DELETE SET NULL;

-- 自己不能是自己的父属性。**自己可以是自己的逆**——那等于对称，
-- 是合法的声明（R0 会提示改用 `symmetric` 更直白，但不算错）
ALTER TABLE relation_types
    ADD CONSTRAINT relation_types_sub_property_not_self
        CHECK (sub_property_of IS NULL OR sub_property_of <> id);

-- 归一化要按「谁指着我」反查，编译规则时每个谓词问一次
CREATE INDEX relation_types_inverse_idx ON relation_types (inverse_of)
    WHERE inverse_of IS NOT NULL;
CREATE INDEX relation_types_sub_property_idx ON relation_types (sub_property_of)
    WHERE sub_property_of IS NOT NULL;

-- 两处 CHECK 跟着放开：新规则与新缺陷都是 0013 那两张表没预见到的取值。
--
-- **不是补漏，是那时确实还没有。** 0013 的注释写着「`inverseOf` 与
-- `subPropertyOf` 投影侧还没落库，所以也就编不出来」——这条迁移补上投影侧，
-- 约束自然要跟着扩。
ALTER TABLE rules DROP CONSTRAINT IF EXISTS rules_kind_check;
ALTER TABLE rules ADD CONSTRAINT rules_kind_check
    CHECK (kind IN ('transitive', 'symmetric', 'inverse', 'sub_property'));

-- 三条新的本体自检：自己是自己的逆（等于 symmetric，提示改写）、
-- 逆没指回来（载入时只补空缺不覆盖，所以矛盾留到这里报）、子属性成环
ALTER TABLE ontology_defects DROP CONSTRAINT IF EXISTS ontology_defects_kind_check;
ALTER TABLE ontology_defects ADD CONSTRAINT ontology_defects_kind_check
    CHECK (kind IN (
        'symmetric_and_asymmetric', 'transitive_and_functional',
        'subclass_cycle', 'disjoint_with_ancestor', 'inherits_disjoint',
        'inverse_of_itself', 'inverse_not_mutual', 'sub_property_cycle'
    ));
