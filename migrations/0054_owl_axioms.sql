-- 本体自己声明的公理落库,给一致性检查当判定依据（0002 R0）。
--
-- 解析那一步已经做完（#121）：`TransitiveProperty` / `SymmetricProperty` /
-- `AsymmetricProperty` / `IrreflexiveProperty` / `disjointWith` 现在都投影得出来，
-- 但字段只活在内存里，投影完就丢。这个迁移给它们腾位置。
--
-- **为什么非要有它们**：开发库里 143 对双向事实，`alias_of` 那 14 条是**对的**
-- （别名本来就双向），`produces` 那 88 条几乎肯定是错的。区分这两者的东西只能
-- 来自本体——没有公理，一致性检查就只能拿启发式冒充判定，而 0001 判据 2
-- 写着「本体是引导不是执法」，0002 的整个立论是「用公理裁决」。

-- 属性公理。跟 `functional` / `inverse_functional` 并排——它们本来就是同一族，
-- 只是那两个先落库了。
--
-- **默认 false 而不是 NULL**：OWL 是开放世界，但一致性检查只能按写下来的判。
-- 「没声明」与「声明为否」在这里后果相同——都不构成报矛盾的依据——所以不必
-- 用三态去区分一个不影响行为的差别。
-- **列名带 is_ 前缀不是风格洁癖**：`symmetric` 与 `asymmetric` 都是 Postgres
-- 保留字（`BETWEEN SYMMETRIC`），裸用会在 `ADD COLUMN` 那一行就报语法错。
-- 加引号能绕过去,但那要求之后每一处写这两列的 SQL 都记得加——漏一处就是
-- 运行时才炸。改名一次,后面都不必记。
ALTER TABLE relation_types
    ADD COLUMN is_transitive  BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN is_symmetric   BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN is_asymmetric  BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN is_irreflexive BOOLEAN NOT NULL DEFAULT FALSE;

-- 类互斥。**存成一张表而不是数组列**：要查的问题是「A 与 B 互斥吗」，
-- 那是一次点查；数组列查起来要么全表扫要么建 GIN，而这里的语义就是一条边。
--
-- 与 `entity_type_parents` 同构——同样是类与类之间的一条关系。
CREATE TABLE entity_type_disjoint (
    kb_id UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    a_id  UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    b_id  UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    -- 两个方向各存一行（导入侧已经把公理的对称性展开了）。主键因此天然去重，
    -- 而查询不必关心调用方从哪一头问
    PRIMARY KEY (kb_id, a_id, b_id),
    -- 自己跟自己互斥是无意义的声明，挡在门口比留着让检查去猜好
    CHECK (a_id <> b_id)
);

-- 「跟这个类互斥的有哪些」是唯一的查法（一致性检查拿实体的类去问）
CREATE INDEX entity_type_disjoint_a_idx ON entity_type_disjoint (kb_id, a_id);
