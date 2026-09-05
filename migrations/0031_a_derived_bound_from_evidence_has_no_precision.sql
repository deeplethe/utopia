-- 0022 第 4 条：派生行的两端由求值器按前提**读出来的**区间求交写入——没起点的前提
-- 从它的证据日期起算，结束了不知哪天的到说出它的那份文档为止。这样一端若来自证据
-- 日期而不是原文的日期，它没有精度可言：给它标一个 day 就是在无知的地方填一个
-- 确定的值（`facts.valid_from_precision` 那条注释说的病）。
--
-- 0013 的约束是「有日期当且仅当有精度」。放宽为「有精度必有日期」——反向不再要求。
-- 只放宽派生表：`facts` 上原文给的日期总带着原文的粒度，那边的约束照旧。
ALTER TABLE derived_facts
    DROP CONSTRAINT derived_from_precision_matches_date,
    DROP CONSTRAINT derived_to_precision_matches_date,
    ADD CONSTRAINT derived_from_precision_needs_date
        CHECK (valid_from_precision IS NULL OR valid_from IS NOT NULL),
    ADD CONSTRAINT derived_to_precision_needs_date
        CHECK (valid_to_precision IS NULL OR valid_to IS NOT NULL);
