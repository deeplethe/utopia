-- 推送来的短语正好是一条声明了的关系时，签名由本体判（#1242，0044 决定 3 的修订）。
--
-- 一个 `statements` 来源（0054）推来 `[Statement] validated_by [Test]`，而导入的本体里
-- validated_by 就是一条声明了域 Statement、值域 Test 的关系：短语归一后等于它的键或标签、
-- 两端的类落在域与值域里、且只有这一条对得上，这条签名不问模型直接绑上。判定照旧是
-- 签名上的一行，人看得见、改得了；记成谁判的得说实话——不是代理（没有模型投过票），
-- 也不是人（没有人点过头），是本体：
--   ontology  按声明判的。人的判定它不覆盖；它不再对得上（关系改了名、改了域）时
--             对齐器照常重判
ALTER TABLE phrase_bindings
    DROP CONSTRAINT phrase_bindings_decided_by_check,
    ADD CONSTRAINT phrase_bindings_decided_by_check
        CHECK (decided_by IN ('agent', 'person', 'ontology'));
